//! PyO3 binding layer for the `fast_mail_parser` extension module.
//!
//! The crate intentionally keeps two parallel data models:
//!
//! - [`mail_parser`] is a **PyO3-free core**: `Mail`/`Attachment` are plain Rust
//!   types that hold the parsed message. Because they have no Python dependency,
//!   the parsing logic can be exercised and tested independently of any Python
//!   runtime.
//! - This module is the **PyO3 binding layer**: [`PyMail`]/[`PyAttachment`] wrap
//!   the core types and expose them to Python, converting Rust values into Python
//!   objects (e.g. `Vec<u8>` -> `bytes`).
//!
//! Keeping the split decouples the parsing logic from the Python bindings: the
//! core stays portable and unit-testable, while everything PyO3-specific lives
//! here.

mod mail_parser;

use mailparse::MailParseError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDateTime, PyList, PyString, PyTzInfo};
use pyo3::{create_exception, exceptions, wrap_pyfunction};
use std::collections::HashMap;
use std::num::NonZeroUsize;

create_exception!(fast_mail_parser, ParseError, exceptions::PyException);

// Subtypes of ParseError, so an existing `except ParseError` keeps catching
// everything while callers that care can distinguish the failure. The
// distinction is actionable: a header-section failure usually means the input is
// not an email at all, a structure failure means it is hostile or truncated, and
// a decode failure means one part's transfer encoding is broken while the rest
// of the message may still be worth looking at.
create_exception!(
    fast_mail_parser,
    HeaderParseError,
    ParseError,
    "The header section could not be parsed."
);
create_exception!(
    fast_mail_parser,
    MimeStructureError,
    ParseError,
    "The MIME structure is malformed, or a resource cap was exceeded."
);
create_exception!(
    fast_mail_parser,
    DecodeError,
    ParseError,
    "A part's Content-Transfer-Encoding could not be decoded."
);

/// Classify a parse failure and build the matching Python exception.
///
/// Classification is by `MailParseError` variant, which is exact rather than a
/// heuristic: in mailparse 0.16, transfer-decoding (`body.rs`) only ever yields
/// the three decode variants, and `Generic` is produced solely by the
/// header-parsing paths -- plus the two caps this crate originates itself, which
/// are matched by their named constants.
///
/// Deliberately done here rather than by threading a typed error through the
/// parser: an earlier attempt at that widened the `Result` carried by the
/// per-part loop and the recursive traversal and cost ~30% throughput, which the
/// benchmark gate caught. The classification is cold, so it belongs on the cold
/// side of the boundary.
fn to_py_err(error: MailParseError) -> PyErr {
    let message = format!("Message parsing error: {error}");
    match error {
        MailParseError::Base64DecodeError(_)
        | MailParseError::QuotedPrintableDecodeError(_)
        | MailParseError::EncodingError(_) => DecodeError::new_err(message),
        MailParseError::Generic(detail)
            if detail == mail_parser::ERR_INPUT_TOO_LARGE
                || detail == mail_parser::ERR_MIME_DEPTH =>
        {
            MimeStructureError::new_err(message)
        }
        MailParseError::Generic(_) => HeaderParseError::new_err(message),
    }
}

/// One mailbox from an address header, exposed to Python.
#[pyclass(skip_from_py_object)]
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

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PyAttachment {
    #[pyo3(get)]
    pub mimetype: String,
    pub content: Vec<u8>,
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
        PyBytes::new(py, self.content.as_slice())
    }
}

impl PyAttachment {
    pub(crate) fn from_attachment(attachment: mail_parser::Attachment) -> Self {
        PyAttachment {
            mimetype: attachment.mimetype,
            content: attachment.content,
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
#[pyclass]
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
    #[pyo3(get)]
    pub attachments: Vec<PyAttachment>,
    #[pyo3(get)]
    pub headers: HashMap<String, Vec<String>>,
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
    #[getter]
    fn date_parsed<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDateTime>>> {
        let Some(epoch) = mail_parser::parse_date_epoch(&self.date) else {
            return Ok(None);
        };
        let utc = PyTzInfo::utc(py)?;
        PyDateTime::from_timestamp(py, epoch as f64, Some(&utc)).map(Some)
    }
}

impl PyMail {
    pub(crate) fn from_mail(mail: mail_parser::Mail) -> Self {
        Self {
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
            attachments: mail
                .attachments
                .into_iter()
                .map(PyAttachment::from_attachment)
                .collect(),
            headers: mail.headers,
        }
    }
}

/// Interpret a Python object as a byte buffer for parsing.
///
/// Accepts `bytes` (used as-is) or `str` (decoded as its UTF-8 bytes; ASCII is
/// unchanged because ASCII == its own UTF-8, and non-ASCII code points round-trip
/// correctly instead of being truncated to their low byte). Any other type raises
/// Python `TypeError`.
fn payload_to_bytes(payload: &Py<PyAny>, py: Python<'_>) -> PyResult<Vec<u8>> {
    let obj = payload.bind(py);

    if let Ok(bytes) = obj.cast::<PyBytes>() {
        return Ok(bytes.as_bytes().to_vec());
    }

    if let Ok(text) = obj.cast::<PyString>() {
        if let Ok(text) = text.to_str() {
            return Ok(text.as_bytes().to_vec());
        }
    }

    Err(PyErr::new::<exceptions::PyTypeError, _>(
        "The argument cannot be interpreted as bytes.",
    ))
}

/// Parse a raw email (`bytes` or `str`) into a [`PyMail`].
///
/// Raises `ParseError`, or more precisely one of its subtypes:
/// `HeaderParseError`, `MimeStructureError` or `DecodeError`.
#[pyfunction]
pub fn parse_email(py: Python<'_>, payload: Py<PyAny>) -> PyResult<PyMail> {
    let message = payload_to_bytes(&payload, py)?;

    // The actual parse is pure Rust and never touches the Python interpreter, so
    // release the GIL (`py.detach`) for its duration. This lets other Python
    // threads -- including other `parse_email` calls -- run concurrently instead
    // of serializing on the GIL, which turns multi-threaded parsing throughput
    // from single-core into multi-core. `message` is an owned copy, so nothing
    // borrows from a Python object while the GIL is released. Errors and the
    // `PyMail` are produced after re-attaching, where the interpreter is needed.
    let mail = py
        .detach(|| mail_parser::parse_email(message.as_slice()))
        .map_err(to_py_err)?;

    Ok(PyMail::from_mail(mail))
}

/// Parse a batch of messages in one call, in parallel, preserving input order.
///
/// Accepts the same `str`/`bytes` inputs as `parse_email`. Each slot of the
/// result is either a `PyMail` or a `ParseError` **instance** -- returned, not
/// raised -- so one malformed message cannot cost the caller the rest of the
/// batch, and inputs zip cleanly to outcomes. `raise_on_error=True` restores
/// fail-fast behaviour for callers who prefer it.
///
/// `threads` caps the worker count; the default is the machine's parallelism.
///
/// Memory: every parsed message is materialised before returning. A batch of ten
/// thousand one-megabyte mails holds essentially all of it decoded at once, so
/// chunk large workloads at the caller.
#[pyfunction]
#[pyo3(signature = (payloads, *, threads = None, raise_on_error = false))]
pub fn parse_many(
    py: Python<'_>,
    payloads: Vec<Py<PyAny>>,
    threads: Option<usize>,
    raise_on_error: bool,
) -> PyResult<Py<PyList>> {
    // Convert every payload to owned bytes *before* releasing the GIL: this
    // touches Python objects, and nothing may borrow from one while detached.
    let messages: Vec<Vec<u8>> = payloads
        .iter()
        .map(|payload| payload_to_bytes(payload, py))
        .collect::<PyResult<_>>()?;

    let workers = threads.and_then(NonZeroUsize::new);

    // The whole batch parses with the GIL released, so other Python threads keep
    // running for its full duration rather than per message.
    let parsed = py.detach(|| mail_parser::parse_many(&messages, workers));

    let items = PyList::empty(py);
    for result in parsed {
        match result {
            Ok(mail) => items.append(Py::new(py, PyMail::from_mail(mail))?)?,
            Err(error) => {
                let err = to_py_err(error);
                if raise_on_error {
                    return Err(err);
                }
                // The exception object itself, not a raise.
                items.append(err.value(py))?;
            }
        }
    }
    Ok(items.unbind())
}

#[pymodule]
fn fast_mail_parser(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(parse_email, m)?)?;
    m.add_function(wrap_pyfunction!(parse_many, m)?)?;
    m.add_class::<PyMail>()?;
    m.add_class::<PyAttachment>()?;
    m.add_class::<PyAddress>()?;
    m.add("ParseError", py.get_type::<ParseError>())?;
    m.add("HeaderParseError", py.get_type::<HeaderParseError>())?;
    m.add("MimeStructureError", py.get_type::<MimeStructureError>())?;
    m.add("DecodeError", py.get_type::<DecodeError>())?;

    Ok(())
}
