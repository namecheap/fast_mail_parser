//! `mode="metadata"`: headers and an attachment inventory, nothing decoded (#233).

use pyo3::prelude::*;
use pyo3::types::{PyDateTime, PyDict};

use crate::convert::{addresses, date_parsed, Headers};
use crate::flat::PyAddress;
use crate::mail_parser;

/// A non-body part described but not decoded, from `mode="metadata"` (#97).
///
/// The same fields as `PyAttachment` minus `content`, plus `encoded_size`. It is
/// a separate type rather than a `PyAttachment` with `content = None`, so that
/// `PyAttachment.content` stays `bytes` for every caller who never asked for this
/// mode -- widening it to `bytes | None` would have broken every `mypy --strict`
/// consumer of the default path.
#[pyclass(frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyAttachmentMetadata {
    #[pyo3(get)]
    pub mimetype: String,
    #[pyo3(get)]
    pub filename: String,
    #[pyo3(get)]
    pub content_id: Option<String>,
    #[pyo3(get)]
    pub disposition: Option<String>,
    /// Bytes this part occupies in the message, **before** transfer-decoding.
    ///
    /// Named for what it is. A bare `size` would be read as the decoded size,
    /// which metadata mode cannot know without doing the decode it exists to
    /// skip: base64 inflates by about a third. In full mode the decoded size is
    /// `len(content)`.
    #[pyo3(get)]
    pub encoded_size: usize,
}

impl PyAttachmentMetadata {
    pub(crate) fn from_metadata(attachment: mail_parser::AttachmentMetadata) -> Self {
        PyAttachmentMetadata {
            mimetype: attachment.mimetype,
            filename: attachment.filename,
            content_id: attachment.content_id,
            disposition: attachment.disposition,
            encoded_size: attachment.encoded_size,
        }
    }
}

/// What a message says about itself, without decoding what it carries (#97).
///
/// Returned by `parse_email(payload, mode="metadata")`. Headers, subject, date
/// and addresses are identical to full mode; attachments are described but not
/// decoded.
///
/// It has no `text_plain`/`text_html` on purpose. #97 proposed empty lists, and
/// an empty list cannot be told apart from "this message has no text part" -- a
/// triage sweep counting bodyless messages would count all of them, which is the
/// same class of silent-wrong-answer as #150. A missing attribute fails loudly.
/// For structure without decoding, `parse_email_tree` is the API that keeps it.
#[pyclass(frozen, skip_from_py_object)]
pub struct PyMailMetadata {
    #[pyo3(get)]
    pub subject: String,
    #[pyo3(get)]
    pub date: String,
    #[pyo3(get)]
    pub from_: Option<PyAddress>,
    #[pyo3(get)]
    pub to: Vec<PyAddress>,
    #[pyo3(get)]
    pub cc: Vec<PyAddress>,
    #[pyo3(get)]
    pub bcc: Vec<PyAddress>,
    #[pyo3(get)]
    pub reply_to: Vec<PyAddress>,
    #[pyo3(get)]
    pub attachments: Vec<PyAttachmentMetadata>,
    pub(crate) headers: Headers,
}

#[pymethods]
impl PyMailMetadata {
    /// Headers, every value kept, keys in wire order -- as in `PyMail` (#157).
    #[getter]
    fn headers<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        self.headers.to_dict(py)
    }

    /// The `Date` header as an aware `datetime`, or `None` if unparseable.
    ///
    /// Present here because sorting or bucketing a sweep by date is most of what
    /// metadata mode is for.
    #[getter]
    fn date_parsed<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDateTime>>> {
        date_parsed(py, &self.date)
    }

    fn __repr__(&self) -> String {
        format!(
            "<PyMailMetadata {:?} attachments={}>",
            self.subject,
            self.attachments.len()
        )
    }
}

impl PyMailMetadata {
    #[inline(never)]
    pub(crate) fn from_metadata(metadata: mail_parser::MailMetadata) -> Self {
        PyMailMetadata {
            subject: metadata.subject,
            date: metadata.date,
            from_: metadata.from_.map(PyAddress::from_address),
            to: addresses(metadata.to),
            cc: addresses(metadata.cc),
            bcc: addresses(metadata.bcc),
            reply_to: addresses(metadata.reply_to),
            attachments: metadata
                .attachments
                .into_iter()
                .map(PyAttachmentMetadata::from_metadata)
                .collect(),
            headers: Headers::new(metadata.headers),
        }
    }
}
