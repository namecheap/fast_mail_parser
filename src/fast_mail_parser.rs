mod mail_parser;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3::{create_exception, exceptions, wrap_pyfunction};
use std::collections::HashMap;

create_exception!(fast_mail_parser, ParseError, exceptions::PyException);

#[pyclass]
#[derive(Clone)]
pub struct PyAttachment {
    #[pyo3(get)]
    pub mimetype: String,
    #[pyo3(get)]
    pub content: Py<PyBytes>,
    #[pyo3(get)]
    pub filename: String,
}

impl PyAttachment {
    pub fn from_attachment(py: Python, attachment: mail_parser::Attachment) -> Self {
        PyAttachment {
            mimetype: attachment.mimetype,
            content: Py::from(PyBytes::new(py, attachment.content.as_slice())),
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

trait PyToBytes {
    fn to_bytes(&self, py: Python) -> PyResult<Vec<u8>>;
}

impl PyToBytes for PyObject {
    fn to_bytes(&self, py: Python) -> PyResult<Vec<u8>> {
        let mut result = self
            .extract::<&PyBytes>(py)
            .map(|s| s.as_bytes().to_vec().into_iter());

        if result.is_err() {
            result = self
                .extract::<String>(py)
                .map(|s| s.chars().map(|c| c as u8).collect::<Vec<_>>().into_iter())
                .map_err(|_| {
                    PyErr::new::<exceptions::PyTypeError, _>(
                        "The argument cannot be interpreted as bytes.",
                    )
                })
        }

        result.map(|iter| iter.collect())
    }
}

#[pyfunction]
pub fn parse_email(py: Python, payload: PyObject) -> PyResult<PyMail> {
    let message = payload.to_bytes(py)?;

    mail_parser::parse_email(message.as_slice())
        .map_err(|e| ParseError::new_err(format!("Message parsing error: {}", e)))
        .map(|m| PyMail {
            subject: m.get_subject(),
            text_plain: m.get_text_plain(),
            text_html: m.get_text_html(),
            date: m.get_date(),
            attachments: m
                .get_attachments()
                .into_iter()
                .map(|a| PyAttachment::from_attachment(py, a))
                .collect(),
            headers: m.get_headers(),
        })
}

#[pymodule]
fn fast_mail_parser(py: Python, m: &PyModule) -> PyResult<()> {
    m.add_wrapped(wrap_pyfunction!(parse_email))?;
    m.add_class::<PyMail>()?;
    m.add_class::<PyAttachment>()?;
    m.add("ParseError", py.get_type::<ParseError>())?;

    Ok(())
}
