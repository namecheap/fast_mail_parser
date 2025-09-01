use mailparse::*;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3::{create_exception, exceptions, wrap_pyfunction};
use std::collections::HashMap;

create_exception!(fast_mail_parser, ParseError, exceptions::PyException);

pub(crate) fn parse_email_raw(payload: &[u8]) -> Result<Mail, MailParseError> {
    Mail::new(payload)
}

#[derive(Debug)]
pub(crate) struct Mail {
    pub(crate) subject: String,
    pub(crate) text_plain: Vec<String>,
    pub(crate) text_html: Vec<String>,
    pub(crate) date: String,
    pub(crate) attachments: Vec<Attachment>,
    pub(crate) headers: HashMap<String, String>,
}

#[derive(Debug)]
pub(crate) struct Attachment {
    pub(crate) mimetype: String,
    pub(crate) content: Vec<u8>,
    pub(crate) filename: String,
}

impl<'a> Mail {
    pub(crate) fn new(payload: &'a [u8]) -> Result<Self, MailParseError> {
        let mail = parse_mail(payload)?;

        let headers: HashMap<String, String> = mail
            .get_headers()
            .into_iter()
            .map(|h| (h.get_key(), h.get_value()))
            .collect();

        let subject = headers.get("Subject").map(String::from).unwrap_or_default();

        let date = headers.get("Date").map(String::from).unwrap_or_default();

        let mut attachments = vec![];
        let mut text_plain = vec![];
        let mut text_html = vec![];

        for mail in Self::extract_mail_parts(mail) {
            let attachment_name = mail.ctype.params.get("name");
            let mime = mail.ctype.mimetype.as_str();

            attachments.push(Attachment {
                mimetype: mime.to_string(),
                content: mail.get_body_raw().unwrap_or_default(),
                filename: attachment_name.cloned().unwrap_or_default(),
            });

            if attachment_name.is_none() {
                if mime == "text/plain" {
                    text_plain.push(mail.get_body().unwrap_or_default())
                } else if mime == "text/html" {
                    text_html.push(mail.get_body().unwrap_or_default())
                }
            }
        }

        Ok(Self {
            subject,
            text_plain,
            text_html,
            date,
            attachments,
            headers,
        })
    }

    fn extract_mail_parts(mut mail: ParsedMail<'a>) -> Vec<ParsedMail<'a>> {
        let mut result = vec![];
        let subparts = std::mem::take(&mut mail.subparts);

        for part in subparts {
            result.extend(Self::extract_mail_parts(part));
        }

        result.push(mail);

        result
    }
}

#[pyclass]
pub struct PyAttachment {
    #[pyo3(get)]
    pub mimetype: String,
    #[pyo3(get)]
    pub content: Py<PyBytes>,
    #[pyo3(get)]
    pub filename: String,
}

impl Clone for PyAttachment {
    fn clone(&self) -> Self {
        Python::with_gil(|py| PyAttachment {
            mimetype: self.mimetype.clone(),
            content: self.content.clone_ref(py),
            filename: self.filename.clone(),
        })
    }
}

impl PyAttachment {
    pub(crate) fn from_attachment(py: Python, attachment: Attachment) -> Self {
        PyAttachment {
            mimetype: attachment.mimetype,
            content: PyBytes::new_bound(py, attachment.content.as_slice()).into(),
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
    pub(crate) fn from_mail(py: Python, mail: Mail) -> Self {
        Self {
            subject: mail.subject,
            text_plain: mail.text_plain,
            text_html: mail.text_html,
            date: mail.date,
            attachments: mail
                .attachments
                .into_iter()
                .map(|a| PyAttachment::from_attachment(py, a))
                .collect(),
            headers: mail.headers,
        }
    }
}

trait PyToBytes {
    fn to_bytes(&self, py: Python) -> PyResult<Vec<u8>>;
}

impl PyToBytes for Bound<'_, PyAny> {
    fn to_bytes(&self, _py: Python) -> PyResult<Vec<u8>> {
        if let Ok(bytes) = self.downcast::<PyBytes>() {
            return Ok(bytes.as_bytes().to_vec());
        }

        if let Ok(string) = self.extract::<String>() {
            return Ok(string.into_bytes());
        }

        Err(PyErr::new::<exceptions::PyTypeError, _>(
            "The argument cannot be interpreted as bytes.",
        ))
    }
}

#[pyfunction]
pub fn parse_email(py: Python, payload: Bound<'_, PyAny>) -> PyResult<PyMail> {
    let message = payload.to_bytes(py)?;

    parse_email_raw(message.as_slice())
        .map_err(|e| ParseError::new_err(format!("Message parsing error: {}", e)))
        .map(|mail| PyMail::from_mail(py, mail))
}

/// A Python module implemented in Rust. The name of this function must match
/// the `lib.name` setting in the `Cargo.toml`, else Python will not be able to
/// import the module.
#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(parse_email, m)?)?;
    m.add_class::<PyMail>()?;
    m.add_class::<PyAttachment>()?;
    m.add("ParseError", m.py().get_type_bound::<ParseError>())?;
    Ok(())
}
