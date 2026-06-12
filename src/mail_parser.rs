//! PyO3-free parsing core for `fast_mail_parser`.
//!
//! This module holds the pure-Rust data model -- [`Mail`] and [`Attachment`] --
//! and the logic that turns a raw message into them. It has no dependency on
//! Python or PyO3, so it can be exercised and unit-tested independently of any
//! Python runtime.
//!
//! The companion `fast_mail_parser` module is the **PyO3 binding layer**:
//! `PyMail`/`PyAttachment` wrap these core types and convert them into Python
//! objects. Keeping the two models separate decouples the parsing logic from the
//! Python bindings.

use charset::{decode_ascii, Charset};
use mailparse::*;
use std::collections::HashMap;

// DoS hardening: `parse_email` runs on untrusted input. The two constants below
// bound otherwise-unbounded resource use. Both limits sit far above any
// realistic email, so well-formed messages are never affected.

// Reject payloads larger than 100 MiB. A single email this large is not
// legitimate; rejecting up front prevents a huge payload from exhausting memory.
const MAX_INPUT_BYTES: usize = 100 * 1024 * 1024;

// Cap MIME multipart nesting at 256 levels. `extract_mail_parts` recurses over
// subparts, so a maliciously deep multipart tree could otherwise blow the stack
// and crash the host process. Real messages nest only a handful of levels deep.
const MAX_MIME_DEPTH: usize = 256;

pub(crate) fn parse_email(payload: &[u8]) -> Result<Mail, MailParseError> {
    Mail::new(payload)
}

/// Decode already-transfer-decoded `body` bytes into a `String` using the part's
/// charset (defaulting to us-ascii when the label is missing or unrecognized).
///
/// This mirrors mailparse's internal `get_body_as_string` exactly -- same crate,
/// same logic -- so it can be fed the bytes from `get_body_raw` to produce the
/// same result as `get_body` without decoding the transfer encoding twice.
fn decode_charset(body: &[u8], ctype: &ParsedContentType) -> String {
    if let Some(charset) = Charset::for_label(ctype.charset.as_bytes()) {
        charset.decode(body).0.into_owned()
    } else {
        decode_ascii(body).into_owned()
    }
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
        if payload.len() > MAX_INPUT_BYTES {
            return Err(MailParseError::Generic(
                "Input exceeds maximum allowed size",
            ));
        }

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

        for mail in Self::extract_mail_parts(mail, 0)? {
            let attachment_name = mail.ctype.params.get("name");
            let mime = mail.ctype.mimetype.as_str();

            // Undo the Content-Transfer-Encoding (e.g. base64/quoted-printable)
            // exactly once. `?` propagates a broken transfer encoding instead of
            // swallowing it with `unwrap_or_default()`, which would silently turn
            // corruption into an empty body; the PyO3 layer surfaces the error to
            // Python as `ParseError`.
            let content = mail.get_body_raw()?;

            // For text parts, build the Python-facing string from the bytes we
            // just decoded instead of calling `get_body()`, which would re-run the
            // identical transfer decode a second time. `decode_charset` performs
            // only the charset step, so the result matches mailparse's `get_body`
            // output byte-for-byte (see `decode_charset`).
            if attachment_name.is_none() {
                if mime == "text/plain" {
                    text_plain.push(decode_charset(&content, &mail.ctype));
                } else if mime == "text/html" {
                    text_html.push(decode_charset(&content, &mail.ctype));
                }
            }

            attachments.push(Attachment {
                mimetype: mime.to_string(),
                content,
                filename: attachment_name.cloned().unwrap_or_default(),
            });
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

    fn extract_mail_parts(
        mut mail: ParsedMail<'a>,
        depth: usize,
    ) -> Result<Vec<ParsedMail<'a>>, MailParseError> {
        if depth >= MAX_MIME_DEPTH {
            return Err(MailParseError::Generic(
                "MIME nesting exceeds maximum allowed depth",
            ));
        }

        let mut result = vec![];
        let subparts = std::mem::take(&mut mail.subparts);

        for part in subparts {
            result.extend(Self::extract_mail_parts(part, depth + 1)?);
        }

        result.push(mail);

        Ok(result)
    }
}
