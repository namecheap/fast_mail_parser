use mailparse::*;
use std::collections::HashMap;

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
        let mail = parse_mail(payload)?;

        let headers: HashMap<String, String> = mail
            .get_headers()
            .into_iter()
            .map(|h| (h.get_key(), h.get_value()))
            .collect();

        let subject = headers
            .get("Subject")
            .map(String::from)
            .unwrap_or_default();

        let date = headers
            .get("Date")
            .map(String::from)
            .unwrap_or_default();

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
