mod mail_parser;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
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

trait PyToBytes {
    fn to_bytes(&self, py: Python<'_>) -> PyResult<Vec<u8>>;
}

impl PyToBytes for Py<PyAny> {
    fn to_bytes(&self, py: Python<'_>) -> PyResult<Vec<u8>> {
        let obj = self.bind(py);

        if let Ok(bytes) = obj.cast::<PyBytes>() {
            return Ok(bytes.as_bytes().to_vec());
        }

        obj.extract::<String>()
            .map(|s| s.chars().map(|c| c as u8).collect())
            .map_err(|_| {
                PyErr::new::<exceptions::PyTypeError, _>(
                    "The argument cannot be interpreted as bytes.",
                )
            })
    }
}

#[pyfunction]
pub fn parse_email(py: Python<'_>, payload: Py<PyAny>) -> PyResult<PyMail> {
    let message = payload.to_bytes(py)?;

    mail_parser::parse_email(message.as_slice())
        .map_err(|e| ParseError::new_err(format!("Message parsing error: {}", e)))
        .map(PyMail::from_mail)
}

#[pymodule]
fn fast_mail_parser(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(parse_email, m)?)?;
    m.add_class::<PyMail>()?;
    m.add_class::<PyAttachment>()?;
    m.add("ParseError", py.get_type::<ParseError>())?;

    Ok(())
}
