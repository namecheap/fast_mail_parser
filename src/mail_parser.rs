//! PyO3-free parsing core for `fast_mail_parser`.
//!
//! This module holds the pure-Rust data model -- [`Mail`] and [`Attachment`] --
//! and the logic that turns a raw message into them. It has no dependency on
//! Python or PyO3, so it can be exercised and unit-tested independently of any
//! Python runtime.
//!
//! ## Thread-safety invariant
//!
//! This module holds **no shared mutable state**: no `static mut`, no
//! `OnceCell`/`OnceLock`, no `thread_local`, no interior mutability, and no
//! `unsafe`. Every `static` here is a `const`. `parse_email` is a pure function
//! of `&[u8]`, and `parse_many`'s workers each keep their own results,
//! coordinating only through an atomic cursor.
//!
//! That is load-bearing, not incidental. It is what makes the free-threaded
//! safety audit on issue #101 hold, and it is why `parse_many` needs no locking.
//! **Introducing shared mutable state here -- a decode cache being the obvious
//! candidate, see issue #97 -- invalidates that audit and must re-open it.**
//!
//! The companion `fast_mail_parser` module is the **PyO3 binding layer**:
//! `PyMail`/`PyAttachment` wrap these core types and convert them into Python
//! objects. Keeping the two models separate decouples the parsing logic from the
//! Python bindings.

use charset::{decode_ascii, Charset};
use mailparse::body::Body;
use mailparse::*;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

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

/// Parse a batch of messages in parallel, preserving input order.
///
/// One result per input, each independently `Ok` or `Err`, so a single malformed
/// message cannot fail the batch.
///
/// Uses `std::thread::scope` and a shared atomic cursor rather than a thread
/// pool crate. Two reasons: it adds no dependency -- which keeps the lockfile
/// and the licence allowlist untouched -- and the cursor gives *dynamic* work
/// distribution, which is the property that actually matters here. Static
/// chunking would stall a worker that happened to draw several large messages,
/// and real mail batches are very uneven in size.
///
/// `threads` caps the worker count; `None` uses the machine's parallelism.
/// Callers with a batch smaller than the thread count do not spawn idle workers.
pub(crate) fn parse_many<P: AsRef<[u8]> + Sync>(
    payloads: &[P],
    threads: Option<NonZeroUsize>,
) -> Vec<Result<Mail, MailParseError>> {
    if payloads.is_empty() {
        return Vec::new();
    }

    let available = threads
        .or_else(|| thread::available_parallelism().ok())
        .map_or(1, NonZeroUsize::get);
    // Never more workers than there is work for them to do.
    let workers = available.min(payloads.len()).max(1);

    if workers == 1 {
        return payloads
            .iter()
            .map(|payload| Mail::new(payload.as_ref()))
            .collect();
    }

    let cursor = AtomicUsize::new(0);
    let collected: Vec<Vec<(usize, Result<Mail, MailParseError>)>> = thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    // Each worker claims indices until the batch is exhausted and
                    // keeps its own results, so no synchronisation is needed on
                    // the output and no `unsafe` is involved.
                    let mut mine = Vec::new();
                    loop {
                        let index = cursor.fetch_add(1, Ordering::Relaxed);
                        if index >= payloads.len() {
                            break;
                        }
                        mine.push((index, Mail::new(payloads[index].as_ref())));
                    }
                    mine
                })
            })
            .collect();

        handles
            .into_iter()
            // A worker only panics if the parser does, which is a bug rather
            // than a malformed-input case. Resuming the unwind keeps the
            // behaviour identical to the single-message path, where PyO3 turns a
            // panic into a Python exception instead of losing it.
            .map(|handle| match handle.join() {
                Ok(results) => results,
                Err(payload) => std::panic::resume_unwind(payload),
            })
            .collect()
    });

    // Restore input order. Slots are filled exactly once, so no gaps.
    let mut ordered: Vec<Option<Result<Mail, MailParseError>>> =
        (0..payloads.len()).map(|_| None).collect();
    for (index, result) in collected.into_iter().flatten() {
        ordered[index] = Some(result);
    }
    ordered
        .into_iter()
        .map(|slot| slot.expect("every index is claimed exactly once"))
        .collect()
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

/// Every value of every header, keyed by name, in first-appearance key order.
///
/// Repeated keys keep all their values: collapsing to one kept only the last,
/// discarding all but the final `Received`, `DKIM-Signature`, `Received-SPF`,
/// ... -- which made delivery-path tracing and signature verification impossible
/// (#12, #23).
///
/// A `HashMap` cannot carry the key order: its iteration order is randomised per
/// instance, and that order became the Python dict's insertion order, so headers
/// came back differently ordered on every parse of the same bytes (#157).
///
/// `positions` keeps insertion O(1). Scanning the vector for each field would be
/// quadratic in the header count, which turns a message carrying thousands of
/// headers into an amplification vector -- the sort of thing MAX_INPUT_BYTES and
/// MAX_MIME_DEPTH exist to prevent elsewhere.
///
/// Shared by the flat view and the tree so the two cannot disagree about what a
/// message's headers are.
fn collect_headers(part: &ParsedMail<'_>) -> Vec<(String, Vec<String>)> {
    let mut headers: Vec<(String, Vec<String>)> = Vec::new();
    let mut positions: HashMap<String, usize> = HashMap::new();

    for header in part.get_headers() {
        let key = header.get_key();
        match positions.get(&key).copied() {
            Some(position) => headers[position].1.push(header.get_value()),
            None => {
                positions.insert(key.clone(), headers.len());
                headers.push((key, vec![header.get_value()]));
            }
        }
    }

    headers
}

/// Size of a part's body as it sits on the wire, before transfer-decoding.
///
/// Cheap: `get_body_encoded` hands back a view of the existing bytes rather than
/// decoding them, which is the whole point in metadata mode.
///
/// `get_raw` lives on the variant payloads and not on `Body` itself, so this has
/// to match. The or-patterns are grouped by payload type: `Base64` and
/// `QuotedPrintable` both carry an `EncodedBody`, `SevenBit` and `EightBit` a
/// `TextBody`.
fn encoded_size(part: &ParsedMail<'_>) -> usize {
    match part.get_body_encoded() {
        Body::Base64(body) | Body::QuotedPrintable(body) => body.get_raw().len(),
        Body::SevenBit(body) | Body::EightBit(body) => body.get_raw().len(),
        Body::Binary(body) => body.get_raw().len(),
    }
}

/// A non-body part, described but not decoded (#97).
#[derive(Debug)]
pub(crate) struct AttachmentMetadata {
    pub(crate) mimetype: String,
    pub(crate) filename: String,
    pub(crate) content_id: Option<String>,
    pub(crate) disposition: Option<String>,
    pub(crate) encoded_size: usize,
}

/// What a message says about itself, without decoding what it carries (#97).
///
/// Deliberately has no `text_plain`/`text_html`. The issue proposed empty lists,
/// and an empty list is indistinguishable from "this message has no text part" --
/// a triage sweep counting bodyless messages would count every message. Absent
/// attributes fail loudly instead. For structure without decoding, use the tree
/// API (#99).
#[derive(Debug)]
pub(crate) struct MailMetadata {
    pub(crate) subject: String,
    pub(crate) date: String,
    pub(crate) from_: Option<Address>,
    pub(crate) to: Vec<Address>,
    pub(crate) cc: Vec<Address>,
    pub(crate) bcc: Vec<Address>,
    pub(crate) reply_to: Vec<Address>,
    pub(crate) attachments: Vec<AttachmentMetadata>,
    pub(crate) headers: Vec<(String, Vec<String>)>,
}

/// Parse headers and the attachment inventory, decoding nothing.
///
/// The MIME tree is still walked -- a part inventory is cheap -- but no
/// transfer-decoding happens and no content is copied, which on an
/// attachment-heavy message is nearly all of the work.
///
/// The envelope extraction below repeats `Mail::new`'s, deliberately: both are
/// only calls into the shared helpers (`collect_headers`, `parse_addresses`), so
/// what is duplicated is the list of headers to read, not any logic. Threading a
/// mode through `Mail::new` instead would have put a branch in the hot path for
/// the benefit of the cold one.
pub(crate) fn parse_email_metadata(payload: &[u8]) -> Result<MailMetadata, MailParseError> {
    if payload.len() > MAX_INPUT_BYTES {
        return Err(MailParseError::Generic(ERR_INPUT_TOO_LARGE));
    }

    let mail = parse_mail(payload)?;
    let headers = collect_headers(&mail);

    let subject = mail
        .get_headers()
        .get_first_value("Subject")
        .unwrap_or_default();
    let date = mail
        .get_headers()
        .get_first_value("Date")
        .unwrap_or_default();
    let from_ = parse_addresses(mail.get_headers().get_first_header("From"))
        .into_iter()
        .next();
    let to = parse_addresses(mail.get_headers().get_first_header("To"));
    let cc = parse_addresses(mail.get_headers().get_first_header("Cc"));
    let bcc = parse_addresses(mail.get_headers().get_first_header("Bcc"));
    let reply_to = parse_addresses(mail.get_headers().get_first_header("Reply-To"));

    let mut attachments = vec![];

    for part in Mail::extract_mail_parts(mail, 0)? {
        let mime = part.ctype.mimetype.as_str();

        // Structure, not content -- same reasoning as the full parse (#22).
        if mime.starts_with("multipart/") {
            continue;
        }

        let disposition = part.get_content_disposition();

        // The same RFC 2183 rule the full parse applies (#25), so the two modes
        // agree on what an attachment is. A body part is skipped entirely here:
        // reporting it with no content and no size would say less than nothing.
        let is_body = disposition.disposition != DispositionType::Attachment
            && matches!(mime, "text/plain" | "text/html");
        if is_body {
            continue;
        }

        attachments.push(AttachmentMetadata {
            mimetype: mime.to_string(),
            filename: part_filename(&disposition, &part.ctype),
            content_id: part
                .get_headers()
                .get_first_value("Content-ID")
                .map(|raw| normalize_content_id(&raw)),
            disposition: disposition_token(&part, &disposition.disposition),
            encoded_size: encoded_size(&part),
        });
    }

    Ok(MailMetadata {
        subject,
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

/// One node of the MIME tree, as the message actually nests it (#99).
///
/// `Mail` is a flattened projection of this: bodies in one list, attachments in
/// another, containers dropped. Any flattening loses something -- which
/// `text/html` corresponds to which `text/plain` sibling, whether a part was
/// `multipart/alternative` or `multipart/mixed` -- and this keeps it.
#[derive(Debug)]
pub(crate) struct MimePart {
    pub(crate) content_type: String,
    pub(crate) headers: Vec<(String, Vec<String>)>,
    pub(crate) filename: String,
    pub(crate) content_id: Option<String>,
    pub(crate) disposition: Option<String>,
    pub(crate) is_message: bool,
    /// Transfer-decoded bytes of a leaf. `None` for a `multipart/*` container,
    /// whose body is just its children with boundaries between them.
    pub(crate) content: Option<Vec<u8>>,
    pub(crate) children: Vec<MimePart>,
}

impl MimePart {
    fn build(part: &ParsedMail<'_>, depth: usize) -> Result<Self, MailParseError> {
        if depth >= MAX_MIME_DEPTH {
            return Err(MailParseError::Generic(ERR_MIME_DEPTH));
        }

        let mime = part.ctype.mimetype.as_str();
        let disposition = part.get_content_disposition();

        let (content, children) = if mime.starts_with("multipart/") {
            // A container's body is the boundary-delimited concatenation of the
            // children below it, so reporting it as content would report the same
            // bytes twice.
            let children = part
                .subparts
                .iter()
                .map(|child| Self::build(child, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            (None, children)
        } else if mime == "message/rfc822" {
            // An embedded message -- a bounce or a forward, which abuse pipelines
            // are made of. mailparse hands it over as an opaque leaf; parsing it
            // is the difference between "there is a message in here" and being
            // able to read its headers.
            //
            // The nesting counts against the same depth cap, so an onion of
            // forwards cannot recurse further than a multipart tree can.
            let raw = part.get_body_raw()?;
            let inner = {
                let parsed = parse_mail(&raw)?;
                Self::build(&parsed, depth + 1)?
            };
            (Some(raw), vec![inner])
        } else {
            (Some(part.get_body_raw()?), Vec::new())
        };

        Ok(MimePart {
            content_type: mime.to_string(),
            headers: collect_headers(part),
            filename: part_filename(&disposition, &part.ctype),
            content_id: part
                .get_headers()
                .get_first_value("Content-ID")
                .map(|raw| normalize_content_id(&raw)),
            disposition: disposition_token(part, &disposition.disposition),
            is_message: mime == "message/rfc822",
            content,
            children,
        })
    }
}

/// Parse a message into its MIME tree, structure intact.
pub(crate) fn parse_email_tree(payload: &[u8]) -> Result<MimePart, MailParseError> {
    if payload.len() > MAX_INPUT_BYTES {
        return Err(MailParseError::Generic(ERR_INPUT_TOO_LARGE));
    }

    MimePart::build(&parse_mail(payload)?, 0)
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
    pub(crate) headers: Vec<(String, Vec<String>)>,
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

        let headers = collect_headers(&mail);

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
