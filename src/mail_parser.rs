extern crate mailparse;

use std::collections::HashMap;

use self::mailparse::*;

pub fn parse_email(payload: &[u8]) -> Result<Mail, MailParseError> {
    Mail::new(payload)
}

#[derive(Debug)]
pub struct Mail<'a> {
    parts: Vec<ParsedMail<'a>>,
    headers: HashMap<String, String>,
}

#[derive(Debug)]
pub struct Attachment {
    pub mimetype: String,
    pub content: Vec<u8>,
}

impl<'a> Mail<'a> {
    pub fn new(payload: &'a [u8]) -> Result<Self, MailParseError> {
        let mail = parse_mail(payload)?;

        let headers: HashMap<String, String> = mail.get_headers()
            .into_iter()
            .map(|h| (h.get_key(), h.get_value()))
            .collect();

        let parts = Self::extract_mail_parts(mail);

        Ok(Self {
            parts,
            headers,
        })
    }

    pub fn get_subject(&self) -> String {
        self.headers.get("Subject").map_or(String::new(), |h| h.to_string())
    }

    pub fn get_text_plain(&self) -> Vec<String> {
        self.find_parts("text/plain")
    }

    pub fn get_text_html(&self) -> Vec<String> {
        self.find_parts("text/html")
    }

    pub fn get_headers(&self) -> HashMap<String, String> {
        self.headers.clone()
    }

    pub fn get_attachments(&self) -> Vec<Attachment> {
        self.parts.iter().map(|p| Attachment {
            mimetype: p.ctype.mimetype.clone(),
            content: p.get_body_raw().unwrap_or_default(),
        }).collect()
    }

    pub fn get_date(&self) -> String {
        self.headers.get("Date").map_or(String::new(), |h| h.to_string())
    }

    fn extract_mail_parts(mut mail: ParsedMail<'a>) -> Vec<ParsedMail<'a>> {
        let mut result = vec![];
        let subparts = std::mem::replace(&mut mail.subparts, vec![]);

        for part in subparts {
            result.extend(Self::extract_mail_parts(part));
        }

        result.push(mail);

        result
    }

    fn find_parts(&self, part_mimetype: &str) -> Vec<String> {
        self.parts
            .iter()
            .filter(|p| p.ctype.mimetype == part_mimetype && p.ctype.params.get("name").is_none())
            .map(|p| p.get_body().unwrap_or_default())
            .collect()
    }
}
