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
