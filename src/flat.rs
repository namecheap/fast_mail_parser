//! The default flat projection of a message, and the warning channel (#233).

use std::sync::OnceLock;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDateTime, PyDict, PyList};

use crate::convert::{date_parsed, Headers};
use crate::mail_parser;

/// One lossy repair a parse performed, reported rather than raised (#100).
///
/// Read-only, three `str` fields, no interior mutability -- the same shape as
/// [`PyAddress`], so the free-threading invariant recorded in the `mail_parser`
/// module still holds.
#[pyclass(frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct ParseWarning {
    /// A stable token naming what was repaired: `"charset-fallback"`,
    /// `"address-unparseable"`, `"date-unparseable"`. This is the field to
    /// match on; the set grows as repairs become observable, so treat an
    /// unrecognised kind as "something was repaired" rather than as impossible.
    #[pyo3(get)]
    pub kind: String,
    /// Where the affected part landed in the result -- `"text_plain[0]"`,
    /// `"text_html[1]"` -- or `""` when the warning is about the message as a
    /// whole rather than one part.
    ///
    /// A locator into the returned `PyMail` rather than MIME tree coordinates:
    /// `parse_email` hands back a flat projection, and a coordinate naming
    /// structure it has already discarded would be a locator the caller cannot
    /// resolve. `parse_email_tree` is where tree coordinates belong.
    #[pyo3(get)]
    pub part_path: String,
    /// Prose for whoever reads the log. Deliberately not a matching key: the
    /// wording is free to improve, `kind` is not.
    #[pyo3(get)]
    pub detail: String,
}

#[pymethods]
impl ParseWarning {
    fn __repr__(&self) -> String {
        format!(
            "<ParseWarning {} {:?}: {}>",
            self.kind, self.part_path, self.detail
        )
    }
}

impl ParseWarning {
    /// Cold by construction: a well-formed message never reaches this.
    #[inline(never)]
    pub(crate) fn from_warning(warning: mail_parser::Warning) -> Self {
        ParseWarning {
            kind: warning.kind.to_owned(),
            part_path: warning.part_path,
            detail: warning.detail,
        }
    }
}

/// One mailbox from an address header, exposed to Python.
#[pyclass(frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyAddress {
    /// The display name, or `None` when the header carries a bare address.
    ///
    /// RFC 2047 encoded-words are decoded, so a non-ASCII name arrives readable.
    #[pyo3(get)]
    pub display_name: Option<String>,
    /// The `addr-spec` -- the `local@domain` part, without angle brackets.
    #[pyo3(get)]
    pub address: String,
}

impl PyAddress {
    pub(crate) fn from_address(address: mail_parser::Address) -> Self {
        PyAddress {
            display_name: address.display_name,
            address: address.address,
        }
    }
}

#[pyclass(frozen, skip_from_py_object)]
pub struct PyAttachment {
    #[pyo3(get)]
    pub mimetype: String,
    /// Decoded by `parse_email`, so a value rather than a deferred decode; the
    /// lazy type is where that trade is made.
    pub content: Vec<u8>,
    /// The `bytes` handed to Python, built on first read and then shared, so
    /// `a.content is a.content` as it is for `PyLazyAttachment`.
    ///
    /// Unlike the lazy cell -- which decodes *outside* `get_or_init`, for the
    /// deadlock reason spelled out there -- this one may initialise inside it:
    /// the bytes are already decoded, and `PyBytes::new` neither releases the
    /// GIL nor fails, so there is no reentrancy to fear.
    content_py: OnceLock<Py<PyBytes>>,
    #[pyo3(get)]
    pub filename: String,
    /// The part's `Content-ID` with angle brackets stripped, or `None`.
    ///
    /// RFC 2392 `cid:` URLs in an HTML body reference this bracket-less form, so
    /// resolving inline images is a lookup keyed on this value.
    #[pyo3(get)]
    pub content_id: Option<String>,
    /// The part's raw `Content-Disposition` token -- typically `"inline"` or
    /// `"attachment"` -- or `None` when the part declares no such header.
    ///
    /// `None` and `"inline"` are reported distinctly: an absent header is not
    /// the same statement as an explicit `inline`.
    #[pyo3(get)]
    pub disposition: Option<String>,
}

#[pymethods]
impl PyAttachment {
    #[getter]
    fn content<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        self.content_py
            .get_or_init(|| PyBytes::new(py, self.content.as_slice()).unbind())
            .bind(py)
            .clone()
    }
}

impl PyAttachment {
    pub(crate) fn from_attachment(attachment: mail_parser::Attachment) -> Self {
        PyAttachment {
            mimetype: attachment.mimetype,
            content: attachment.content,
            content_py: OnceLock::new(),
            filename: attachment.filename,
            content_id: attachment.content_id,
            disposition: attachment.disposition,
        }
    }
}

/// A parsed email message exposed to Python.
///
/// Body parts and [`attachments`](Self::attachments) are disjoint; `multipart/*`
/// container nodes appear in neither.
#[pyclass(frozen)]
pub struct PyMail {
    #[pyo3(get)]
    pub subject: String,
    #[pyo3(get)]
    pub text_plain: Vec<String>,
    #[pyo3(get)]
    pub text_html: Vec<String>,
    #[pyo3(get)]
    pub date: String,
    /// The `From` mailbox, or `None` when the header is absent or unparseable.
    ///
    /// Named `from_` because `from` is a Python keyword.
    #[pyo3(get)]
    pub from_: Option<PyAddress>,
    /// `To` recipients. RFC 5322 groups are flattened to their members.
    #[pyo3(get)]
    pub to: Vec<PyAddress>,
    /// `Cc` recipients, flattened as `to` is.
    #[pyo3(get)]
    pub cc: Vec<PyAddress>,
    /// `Bcc` recipients, flattened as `to` is. Usually empty on received mail.
    #[pyo3(get)]
    pub bcc: Vec<PyAddress>,
    /// `Reply-To` mailboxes, flattened as `to` is.
    #[pyo3(get)]
    pub reply_to: Vec<PyAddress>,
    /// The message's non-body parts: real attachments and inline resources.
    ///
    /// Per RFC 2183 a part is body text -- and so absent here -- when it is
    /// `text/plain` or `text/html` and is not marked `Content-Disposition:
    /// attachment`. `multipart/*` container nodes are MIME structure and are not
    /// reported. `filename` may still be empty, which is normal for an inline
    /// image referenced only by `Content-ID`.
    ///
    /// Held as Python objects rather than Rust values, for the reason
    /// `PyLazyMail` gives: `#[pyo3(get)]` on a `Vec<pyclass>` goes through
    /// PyO3's clone path, so every read of this attribute used to deep-copy
    /// every attachment's decoded bytes -- and handed back different objects
    /// each time, which would leave the `content` cache with nothing to cache.
    pub attachments: Vec<Py<PyAttachment>>,
    /// Stored as ordered pairs, not a map: the key order is the point (#157).
    /// Exposed through the `headers` getter below.
    pub(crate) headers: Headers,
    /// Every lossy repair this parse performed, in the order it performed them
    /// (#100).
    ///
    /// **Empty means a pristine parse.** That is the guarantee worth having and
    /// the reason this is a list rather than a log line: a pipeline can treat
    /// `warnings == []` as "nothing here was patched up" and route everything
    /// else to quarantine or review. Best-effort parsing was always the
    /// behaviour; this is what makes it observable.
    ///
    /// Cheap when empty by construction, which is the case that matters: the
    /// core builds a `Vec` that does not allocate until something is pushed, and
    /// every push sits behind a branch well-formed mail does not take.
    #[pyo3(get)]
    pub warnings: Vec<ParseWarning>,
}

#[pymethods]
impl PyMail {
    /// `date` parsed to a timezone-aware `datetime`, or `None`.
    ///
    /// Computed on access rather than at parse time: most callers never read it,
    /// and building a Python object for every parsed message would charge them
    /// all for a field they do not use.
    ///
    /// The value is UTC. mailparse resolves the header's offset to an epoch, so
    /// the instant is exact while the original offset is not retained -- read
    /// `date` for that. An unparseable header yields `None`, leaving `date`
    /// intact as the raw string.
    /// All values of every header, keyed by name, in the order the names first
    /// appeared in the message.
    ///
    /// Built on first access and then shared, so every read is the same dict
    /// (#231). Python dicts preserve insertion order, so inserting in wire order
    /// is what makes the ordering observable -- and the previous `HashMap` field,
    /// converted per access, produced a different order every time (#157).
    #[getter]
    fn headers<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        self.headers.to_dict(py)
    }

    /// The message's non-body parts, in message order -- the same objects on
    /// every read, as `PyLazyMail.attachments` hands back the same ones.
    #[getter]
    fn attachments<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        PyList::new(py, self.attachments.iter().map(|a| a.clone_ref(py)))
    }

    #[getter]
    fn date_parsed<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDateTime>>> {
        date_parsed(py, &self.date)
    }
}

impl PyMail {
    pub(crate) fn from_mail(py: Python<'_>, mail: mail_parser::Mail) -> PyResult<Self> {
        // One `Py::new` per attachment, with the GIL held, as lazy mode already
        // does in `PyLazyMail::from_lazy`. Nothing is copied here: the decoded
        // `Vec<u8>` moves into the object and only becomes `bytes` when
        // `content` is first read.
        let attachments = mail
            .attachments
            .into_iter()
            .map(|attachment| Py::new(py, PyAttachment::from_attachment(attachment)))
            .collect::<PyResult<Vec<_>>>()?;

        Ok(Self {
            subject: mail.subject,
            text_plain: mail.text_plain,
            text_html: mail.text_html,
            date: mail.date,
            from_: mail.from_.map(PyAddress::from_address),
            to: mail.to.into_iter().map(PyAddress::from_address).collect(),
            cc: mail.cc.into_iter().map(PyAddress::from_address).collect(),
            bcc: mail.bcc.into_iter().map(PyAddress::from_address).collect(),
            reply_to: mail
                .reply_to
                .into_iter()
                .map(PyAddress::from_address)
                .collect(),
            attachments,
            headers: Headers::new(mail.headers),
            // Empty in the common case, where `collect` allocates nothing.
            warnings: mail
                .warnings
                .into_iter()
                .map(ParseWarning::from_warning)
                .collect(),
        })
    }
}
