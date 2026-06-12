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

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyString};
use pyo3::{create_exception, exceptions, wrap_pyfunction};
use std::collections::HashMap;

create_exception!(fast_mail_parser, ParseError, exceptions::PyException);

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PyAttachment {
    #[pyo3(get)]
    pub mimetype: String,
    pub content: Vec<u8>,
    #[pyo3(get)]
    pub filename: String,
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
        }
    }
}

/// A parsed email message exposed to Python.
///
/// Note that [`attachments`](Self::attachments) is **not** limited to file
/// attachments: it contains every node of the message's MIME tree.
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
    /// Every node of the message's MIME tree, not just file attachments.
    ///
    /// This includes the text parts (`text/plain`, `text/html`, whose decoded
    /// bodies also appear in `text_plain`/`text_html`) and the multipart
    /// container nodes themselves (e.g. `multipart/mixed`, `multipart/alternative`).
    /// Container nodes carry their MIME type but have empty `content`. A part is
    /// only a real file attachment when its `filename` is non-empty.
    #[pyo3(get)]
    pub attachments: Vec<PyAttachment>,
    #[pyo3(get)]
    pub headers: HashMap<String, String>,
}

impl PyMail {
    pub(crate) fn from_mail(mail: mail_parser::Mail) -> Self {
        Self {
            subject: mail.subject,
            text_plain: mail.text_plain,
            text_html: mail.text_html,
            date: mail.date,
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
/// The resulting `PyMail.attachments` lists every node of the MIME tree -- text
/// parts and the multipart container nodes -- not only file attachments; see
/// [`PyMail::attachments`] for details.
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
        .map_err(|e| ParseError::new_err(format!("Message parsing error: {}", e)))?;

    Ok(PyMail::from_mail(mail))
}

#[pymodule]
fn fast_mail_parser(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(parse_email, m)?)?;
    m.add_class::<PyMail>()?;
    m.add_class::<PyAttachment>()?;
    m.add("ParseError", py.get_type::<ParseError>())?;

    Ok(())
}
