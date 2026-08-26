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

// The two cap failures are the only errors this module originates itself;
// everything else comes from mailparse. They are named so the binding layer can
// classify them by identity rather than by re-typing the literals (see
// `to_py_err` in the binding layer).
pub(crate) const ERR_INPUT_TOO_LARGE: &str = "Input exceeds maximum allowed size";
pub(crate) const ERR_MIME_DEPTH: &str = "MIME nesting exceeds maximum allowed depth";

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

/// Resolve a part's filename: RFC 2183 `Content-Disposition; filename` first,
/// falling back to the legacy `Content-Type; name` parameter.
///
/// mailparse lowercases param keys, strips enclosing quotes, and folds RFC 2231
/// extended values (`filename*=utf-8''...`) back into the plain `filename` key,
/// so both lookups below are exact.
fn part_filename(disposition: &ParsedContentDisposition, ctype: &ParsedContentType) -> String {
    disposition
        .params
        .get("filename")
        .or_else(|| ctype.params.get("name"))
        .cloned()
        .unwrap_or_default()
}

/// Strip the angle brackets from a `Content-ID` value, preserving case.
///
/// RFC 2392 `cid:` URLs reference the bracket-less form, so normalizing here is
/// what turns `cid:` resolution into a plain lookup for callers.
fn normalize_content_id(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_string()
}

/// The part's raw `Content-Disposition` token, or `None` when it declares none.
///
/// mailparse defaults the parsed disposition to `Inline` when the header is
/// absent, which is indistinguishable from an explicit
/// `Content-Disposition: inline`, so presence is confirmed against the raw
/// headers before reporting a token.
fn disposition_token(part: &ParsedMail<'_>, kind: &DispositionType) -> Option<String> {
    part.get_headers().get_first_value("Content-Disposition")?;
    Some(match kind {
        DispositionType::Inline => "inline".to_owned(),
        DispositionType::Attachment => "attachment".to_owned(),
        DispositionType::FormData => "form-data".to_owned(),
        DispositionType::Extension(other) => other.clone(),
    })
}

/// Month tokens `mailparse::dateparse` accepts.
///
/// Its state machine only advances past the month once one of these matches,
/// and it returns an error on any other token in that position -- so a
/// *successful* parse with none of these present means the machine never
/// advanced at all. See `parse_date_epoch`.
const MONTH_TOKENS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

/// Parse an RFC 5322 `Date` header to a Unix timestamp, or `None`.
///
/// Kept here rather than in the binding layer so the PyO3-free core owns all
/// parsing; the binding layer only turns the result into a Python object.
///
/// Guards a sharp edge in `dateparse`: for input it never actually parses, its
/// loop simply never advances state and the function still ends in
/// `Ok(result)` with `result` at its initial 0. `dateparse("not a date")` is
/// therefore `Ok(0)`, which would surface garbage to callers as 1970-01-01
/// rather than as nothing at all -- silently wrong being worse than absent.
///
/// Requiring a month token rules that out, because the state machine cannot
/// reach a real result without consuming one. A legitimate epoch-0 date still
/// works: `Thu, 01 Jan 1970 00:00:00 +0000` contains `JAN`.
pub(crate) fn parse_date_epoch(date: &str) -> Option<i64> {
    let upper = date.to_uppercase();
    if !MONTH_TOKENS.iter().any(|month| upper.contains(month)) {
        return None;
    }
    dateparse(date).ok()
}

/// One mailbox from an address header.
#[derive(Debug, Clone)]
pub(crate) struct Address {
    pub(crate) display_name: Option<String>,
    pub(crate) address: String,
}

impl Address {
    fn from_single(info: &SingleInfo) -> Self {
        Self {
            display_name: info.display_name.clone(),
            address: info.addr.clone(),
        }
    }
}

/// Parse one address header into a flat list of mailboxes.
///
/// RFC 5322 groups (`To: team: a@x, b@x;`) are flattened to their members: the
/// group name is structure, and callers want the mailboxes.
///
/// Takes the header rather than its value so `addrparse_header` can tokenize
/// RFC 2047 encoded-words separately from the address syntax. Parsing an
/// already-decoded string would let a decoded display name containing `,` or
/// `<` corrupt the address split.
///
/// A header that fails to parse yields an empty list rather than an error --
/// mailparse rejects an address with no `@`, and a malformed `To:` must not fail
/// an otherwise good message. The raw value stays available through `headers`.
fn parse_addresses(header: Option<&MailHeader<'_>>) -> Vec<Address> {
    let Some(header) = header else {
        return Vec::new();
    };
    let Ok(parsed) = addrparse_header(header) else {
        return Vec::new();
    };

    let mut addresses = Vec::new();
    for entry in parsed.iter() {
        match entry {
            MailAddr::Single(info) => addresses.push(Address::from_single(info)),
            MailAddr::Group(group) => {
                addresses.extend(group.addrs.iter().map(Address::from_single));
            }
        }
    }
    addresses
}

#[derive(Debug)]
pub(crate) struct Mail {
    pub(crate) subject: String,
    pub(crate) text_plain: Vec<String>,
    pub(crate) text_html: Vec<String>,
    pub(crate) date: String,
    pub(crate) from_: Option<Address>,
    pub(crate) to: Vec<Address>,
    pub(crate) cc: Vec<Address>,
    pub(crate) bcc: Vec<Address>,
    pub(crate) reply_to: Vec<Address>,
    pub(crate) attachments: Vec<Attachment>,
    pub(crate) headers: HashMap<String, Vec<String>>,
}

#[derive(Debug)]
pub(crate) struct Attachment {
    pub(crate) mimetype: String,
    pub(crate) content: Vec<u8>,
    pub(crate) filename: String,
    pub(crate) content_id: Option<String>,
    pub(crate) disposition: Option<String>,
}

impl<'a> Mail {
    pub(crate) fn new(payload: &'a [u8]) -> Result<Self, MailParseError> {
        if payload.len() > MAX_INPUT_BYTES {
            return Err(MailParseError::Generic(ERR_INPUT_TOO_LARGE));
        }

        let mail = parse_mail(payload)?;

        // Keep every value for a repeated key, in the order the keys appear.
        // Collapsing to one value per key kept only the last, discarding all but
        // the final `Received`, `DKIM-Signature`, `Received-SPF`, ... -- which
        // made delivery-path tracing and signature verification impossible
        // (#12, #23).
        let mut headers: HashMap<String, Vec<String>> = HashMap::new();
        for header in mail.get_headers() {
            headers
                .entry(header.get_key())
                .or_default()
                .push(header.get_value());
        }

        // Read these straight from the parsed headers rather than back out of the
        // map above, so the dedicated fields do not inherit its representation
        // (#28). `get_first_value` is the first occurrence, which is the correct
        // choice for a header that should appear once.
        let subject = mail
            .get_headers()
            .get_first_value("Subject")
            .unwrap_or_default();
        let date = mail
            .get_headers()
            .get_first_value("Date")
            .unwrap_or_default();

        // Address headers are parsed from their first occurrence, like Subject
        // and Date. `From` is a single mailbox in practice, so it is exposed as
        // one value; the first mailbox is taken if a message declares several.
        let from_ = parse_addresses(mail.get_headers().get_first_header("From"))
            .into_iter()
            .next();
        let to = parse_addresses(mail.get_headers().get_first_header("To"));
        let cc = parse_addresses(mail.get_headers().get_first_header("Cc"));
        let bcc = parse_addresses(mail.get_headers().get_first_header("Bcc"));
        let reply_to = parse_addresses(mail.get_headers().get_first_header("Reply-To"));

        let mut attachments = vec![];
        let mut text_plain = vec![];
        let mut text_html = vec![];

        for part in Self::extract_mail_parts(mail, 0)? {
            let mime = part.ctype.mimetype.as_str();

            // `multipart/*` nodes are MIME structure, not content: their body is
            // the boundary-delimited concatenation of children already visited.
            // Emitting them produced phantom, filename-less `attachments` entries
            // (#22), so skip them before decoding anything.
            if mime.starts_with("multipart/") {
                continue;
            }

            let disposition = part.get_content_disposition();
            let filename = part_filename(&disposition, &part.ctype);

            // RFC 2183 decides body-vs-attachment -- not the media type, and not
            // the mere presence of a filename (#25):
            //   * `Content-Disposition: attachment` means "not for inline
            //     display", so a `text/plain` part marked that way is a file whose
            //     bytes must not be concatenated into the body.
            //   * anything else that is `text/plain` or `text/html` is body text,
            //     even when it carries a `Content-Type; name` parameter. A `name`
            //     alone previously made the body vanish.
            let is_body = disposition.disposition != DispositionType::Attachment
                && matches!(mime, "text/plain" | "text/html");

            // Undo the Content-Transfer-Encoding (e.g. base64/quoted-printable)
            // exactly once. `?` propagates a broken transfer encoding instead of
            // swallowing it with `unwrap_or_default()`, which would silently turn
            // corruption into an empty body; the PyO3 layer surfaces the error to
            // Python as `ParseError`.
            let content = part.get_body_raw()?;

            if !is_body {
                let content_id = part
                    .get_headers()
                    .get_first_value("Content-ID")
                    .map(|raw| normalize_content_id(&raw));

                attachments.push(Attachment {
                    mimetype: mime.to_string(),
                    content,
                    filename,
                    content_id,
                    disposition: disposition_token(&part, &disposition.disposition),
                });
            } else if mime == "text/html" {
                // For text parts, build the Python-facing string from the bytes
                // just decoded rather than calling `get_body()`, which would re-run
                // the identical transfer decode. `decode_charset` performs only the
                // charset step, so the result matches mailparse's `get_body` output
                // byte-for-byte (see `decode_charset`).
                text_html.push(decode_charset(&content, &part.ctype));
            } else {
                // Only `text/plain` reaches here: `is_body` is false for every
                // other media type.
                text_plain.push(decode_charset(&content, &part.ctype));
            }
        }

        Ok(Self {
            subject,
            text_plain,
            text_html,
            date,
            from_,
            to,
            cc,
            bcc,
            reply_to,
            attachments,
            headers,
        })
    }

    fn extract_mail_parts(
        mut mail: ParsedMail<'a>,
        depth: usize,
    ) -> Result<Vec<ParsedMail<'a>>, MailParseError> {
        if depth >= MAX_MIME_DEPTH {
            return Err(MailParseError::Generic(ERR_MIME_DEPTH));
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
