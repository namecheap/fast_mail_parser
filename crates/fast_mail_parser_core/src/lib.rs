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
//! **Introducing shared mutable state here invalidates that audit and must
//! re-open it.**
//!
//! `mode="lazy"` (#97) is the case that note named as the obvious candidate, and
//! it is deliberately not one: this module's contribution to it is
//! [`decode_part`], a pure function of `&[u8]`. The decode-once cache is a
//! `OnceLock` on the *Python object* that owns the retained bytes, in the binding
//! layer, where the object's own lifetime bounds it and PyO3 owns the
//! free-threading question. Nothing here is shared and nothing here is mutable.
//!
//! The parse-warning collector (#100) is deliberately *not* an exception to
//! that: it is a plain `Vec` owned by one `Mail::new` call and threaded by
//! `&mut`, so it is per-call state on the stack of whichever worker is parsing.
//! Nothing is shared, and the audit is untouched.
//!
//! The companion `fast_mail_parser` module is the **PyO3 binding layer**:
//! `PyMail`/`PyAttachment` wrap these core types and convert them into Python
//! objects. Keeping the two models separate decouples the parsing logic from the
//! Python bindings.

// Re-exported so the binding layer can name the error type it converts to a
// Python exception without declaring `mailparse` itself: the version
// requirement for it lives in this crate's manifest and nowhere else.
pub use mailparse::MailParseError;

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

// Below this many input bytes per worker, starting the worker costs more than it
// saves. The README's own cost model puts a parse at roughly 1.1 us per KB, and
// creating plus joining an OS thread is 15-40 us (stack mmap, clone, scheduler
// wake), which puts break-even somewhere near 30 KB. 64 KiB leaves margin for
// slower encodings and slower spawns. A batch with less work than this runs on
// the calling thread, which is the shape a mail pipeline produces most often --
// an IMAP fetch page, a queue poll -- and the shape that used to spawn one
// thread per message to parse about a microsecond each.
const MIN_BYTES_PER_WORKER: usize = 64 * 1024;

// The two cap failures are the only errors this module originates itself;
// everything else comes from mailparse. They are named so the binding layer can
// classify them by identity rather than by re-typing the literals (see
// `to_py_err` in the binding layer).
pub const ERR_INPUT_TOO_LARGE: &str = "Input exceeds maximum allowed size";
pub const ERR_MIME_DEPTH: &str = "MIME nesting exceeds maximum allowed depth";

// Warning kinds (#100). `&'static str` rather than an enum on purpose: the value
// crosses into Python as a string and callers match on it there, so an enum
// would buy a conversion in each direction and nothing else. The binding layer
// maps these to exceptions for `strict=True`, matching on identity here rather
// than re-typing the literals.
pub const KIND_CHARSET_FALLBACK: &str = "charset-fallback";
pub const KIND_ADDRESS_UNPARSEABLE: &str = "address-unparseable";
pub const KIND_DATE_UNPARSEABLE: &str = "date-unparseable";
pub const KIND_UNTERMINATED_HEADERS: &str = "unterminated-header-block";
pub const KIND_TRANSFER_DECODE_LOSSY: &str = "transfer-decode-lossy";

// Held as a const so the helper that uses it is one short line instead of a
// chain across a multi-line literal.
const DETAIL_UNTERMINATED_HEADERS: &str = "the header block was not \
     terminated by an empty line (RFC 5322 2.1); the separator was restored \
     before parsing, so no part was lost -- the stdlib calls this defect \
     MissingHeaderBodySeparatorDefect";

/// Offset of a quoted-printable escape a strict decoder would reject, if any.
///
/// mailparse decodes quoted-printable in robust mode, which passes an invalid
/// escape through as literal text instead of failing. So `=ZZ` survives as three
/// characters where the sender meant one byte, and the parse reports success --
/// a lossy repair with no error, which is what this channel exists for (#100).
///
/// Only the two valid forms are skipped: `=` with two hex digits, and `=` before
/// a line ending (a soft break). Everything else, including a trailing `=` with
/// nothing after it, is what a strict decoder rejects.
///
/// Line-ending canonicalisation is deliberately NOT reported. Robust mode also
/// turns a bare LF into CRLF, which a strict decoder rejects too -- but most mail
/// written with bare LFs would then warn, and a channel whose empty list means
/// something cannot afford that (see the note on `Warning`). The bytes-changed
/// case worth a warning is the one where the sender's intent is lost.
///
/// Parts that are not quoted-printable return immediately. `#[inline(never)]` for
/// the usual reason in this crate: this is called from the per-part loop.
#[inline(never)]
fn quoted_printable_invalid_escape(body: &Body<'_>) -> Option<usize> {
    let Body::QuotedPrintable(body) = body else {
        return None;
    };

    let raw = body.get_raw();

    let mut at = 0;

    while at < raw.len() {
        if raw[at] != b'=' {
            at += 1;
            continue;
        }

        let rest = &raw[at + 1..];
        if rest.starts_with(b"\r\n") || rest.starts_with(b"\n") {
            at += 2;
            continue;
        }
        if rest.len() >= 2 && rest[0].is_ascii_hexdigit() && rest[1].is_ascii_hexdigit() {
            at += 3;
            continue;
        }

        return Some(at);
    }

    None
}

/// One lossy repair the parser performed, recorded instead of raised (#100).
///
/// The empty list is the contract worth having: `warnings == []` says nothing
/// was patched up, which is what lets a consumer route everything else to
/// quarantine. So it has to be exact rather than best-effort -- and it has to
/// cost nothing when there is nothing to report, because that is essentially
/// every message. An empty `Vec` performs no allocation, and every site that
/// builds one of these sits behind a branch well-formed mail never takes.
#[derive(Debug)]
pub struct Warning {
    pub kind: &'static str,
    /// Where the affected part landed in the result -- `"text_plain[0]"` -- or
    /// `""` when the warning is about the message rather than one part.
    pub part_path: String,
    pub detail: String,
}

// Every `warn_*` helper below is `#[cold]` and `#[inline(never)]`, which is
// load-bearing rather than decoration. Each one builds a message, and a `format!`
// inlined into the per-part loop is exactly the shape that cost ~30% on the hot
// path in #135 while never executing. Keeping that out of line leaves the callers
// with a branch and a call they do not take.

#[cold]
#[inline(never)]
fn warn_charset(warnings: &mut Vec<Warning>, field: &str, index: usize, label: &str) {
    warnings.push(Warning {
        kind: KIND_CHARSET_FALLBACK,
        part_path: format!("{field}[{index}]"),
        detail: format!(
            "unrecognised charset {label:?}; the part was decoded as us-ascii, \
             so every non-ASCII byte in it is now U+FFFD"
        ),
    });
}

#[cold]
#[inline(never)]
fn warn_address(warnings: &mut Vec<Warning>, name: &'static str) {
    warnings.push(Warning {
        kind: KIND_ADDRESS_UNPARSEABLE,
        part_path: String::new(),
        detail: format!(
            "the {name} header is not a parseable address list; no mailboxes \
             were reported for it, and its raw value is in headers"
        ),
    });
}

/// Record that the header block had to be resynced before mailparse saw it.
///
/// Inserted at the front rather than pushed: the repair happens before the parse,
/// so it precedes anything the parse itself could report, and `warnings[0]` --
/// which is what strict mode names -- should be the defect that changed the
/// input. The list holds a handful of entries at most and this is the cold path,
/// so the shift costs nothing worth avoiding.
#[cold]
#[inline(never)]
fn warn_separator(warnings: &mut Vec<Warning>) {
    let warning = Warning {
        kind: KIND_UNTERMINATED_HEADERS,
        part_path: String::new(),
        detail: DETAIL_UNTERMINATED_HEADERS.to_owned(),
    };
    warnings.insert(0, warning);
}

#[cold]
#[inline(never)]
fn warn_transfer_decode(warnings: &mut Vec<Warning>, field: &str, index: usize, at: usize) {
    warnings.push(Warning {
        kind: KIND_TRANSFER_DECODE_LOSSY,
        part_path: format!("{field}[{index}]"),
        detail: format!(
            "the quoted-printable escape at byte {at} of the encoded body is \
             neither `=` followed by two hex digits nor a soft line break; it \
             was passed through as literal text rather than decoded"
        ),
    });
}

#[cold]
#[inline(never)]
fn warn_date(warnings: &mut Vec<Warning>, date: &str) {
    warnings.push(Warning {
        kind: KIND_DATE_UNPARSEABLE,
        part_path: String::new(),
        detail: format!(
            "the Date header {date:?} is not a parseable date; date_parsed is \
             None while date keeps the raw value"
        ),
    });
}

/// Restore the header/body separator when a message omits it, or `None` when the
/// message has one and can be parsed exactly as it stands.
///
/// RFC 5322 section 2.1 ends the header block with an empty line. Real mail
/// sometimes omits it -- `tests/data/invalid_message.eml` is a Mailchimp-delivered
/// message that does, and the stdlib names the defect
/// `MissingHeaderBodySeparatorDefect`.
/// mailparse stops parsing headers only at an empty line, and it accepts a line
/// with no colon as a field name with an empty value, so with the separator gone
/// it keeps consuming the body as headers. In that fixture it swallows the first
/// MIME boundary, which leaves the first part's body sitting before the *next*
/// boundary -- making it multipart preamble, discarded by definition -- so the
/// `text/plain` alternative vanished and nothing was raised about it (#150).
///
/// The recovery is the stdlib's: a non-continuation line in the header block that
/// cannot be a header field ends the header block, and the body starts there.
/// Handing mailparse the separator the sender left out is what makes it reach
/// the same conclusion, and it needs no change to the header parsing itself,
/// which mailparse owns.
///
/// "Cannot be a header field" is narrowed here to "contains no colon". The
/// stdlib is stricter -- a field name may hold only printable ASCII other than
/// colon, so `Subject : x`, or an 8-bit byte in a field name, ends the block for
/// it too -- but mailparse accepts both of those as headers, and demoting them to
/// body text would trade one silent loss for another. The colon is the part of
/// the rule that identifies this defect. Two exemptions are the stdlib's own: a
/// line starting with space or tab is a folded continuation, and a leading
/// `From ` line is an mbox envelope header rather than a field.
///
/// What gets repaired is a whole message: the payload handed in, and an embedded
/// `message/rfc822` when the tree parses one. A multipart *part*'s headers are
/// parsed inside mailparse's boundary split, which is not reachable from here, so
/// a part that omits its own separator is still read the way mailparse reads it.
///
/// Cost on a well-formed message is one pass over the header block, ending at the
/// empty line it has: O(header bytes), one comparison per line, and no
/// allocation. Only a defective message allocates, and it allocates once.
///
/// `#[inline(never)]` because this runs on every parse while being nothing worth
/// inlining, and because code size in the parse path is not free in this crate --
/// see the Performance section of CONTRIBUTING.md.
#[inline(never)]
fn repair_missing_separator(payload: &[u8]) -> Option<Vec<u8>> {
    let mut at = 0;

    while at < payload.len() {
        let rest = &payload[at..];
        let newline = rest.iter().position(|&b| b == b'\n');
        let line = &rest[..newline.unwrap_or(rest.len())];

        // An empty line is the separator, in its place: everything past it is
        // body, which this scan never reads. A line starting with CR is either
        // that same separator spelled `\r\n` or a lone CR where a header should
        // start, which mailparse rejects on its own. Neither is ours to repair.
        if line.is_empty() || line.starts_with(b"\r") {
            return None;
        }

        let folded = matches!(line[0], b' ' | b'\t');
        if !folded && !line.starts_with(b"From ") && !line.contains(&b':') {
            // A bare LF, whatever the message's own line endings: mailparse
            // takes a lone LF as the terminator and reports the body as starting
            // after it, and its boundary search accepts a delimiter sitting at
            // the first body byte. The separator is consumed rather than
            // becoming body, so its spelling is not observable either way.
            let mut repaired = Vec::with_capacity(payload.len() + 1);
            repaired.extend_from_slice(&payload[..at]);
            repaired.push(b'\n');
            repaired.extend_from_slice(&payload[at..]);
            return Some(repaired);
        }

        // No newline after this line means no body follows it, so there is no
        // separator missing from between the two.
        at += newline? + 1;
    }

    None
}

pub fn parse_email(payload: &[u8]) -> Result<Mail, MailParseError> {
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
///
/// The batching itself lives in [`parse_many_as`], which the other modes reuse;
/// this is the full-mode call into it (#202).
pub fn parse_many<P: AsRef<[u8]> + Sync>(
    payloads: &[P],
    threads: Option<NonZeroUsize>,
) -> Vec<Result<Mail, MailParseError>> {
    parse_many_as(payloads, threads, Mail::new)
}

/// The scheduler above, with the per-message parse as a parameter (#202).
///
/// `mode=` on `parse_many` needs the identical batching for three different
/// result types, and the batching is the part worth not having two copies of:
/// the dynamic cursor and the order restoration are where a bug would be subtle,
/// while the per-message parse is a single call. So the modes vary the call and
/// share everything around it.
///
/// A `fn` pointer rather than `impl Fn`, so the number of instantiations is the
/// number of result types and not also the number of call sites. Each of the
/// three parse functions is a plain `fn` item, and the indirect call happens once
/// per message around a whole parse.
pub fn parse_many_as<P, T>(
    payloads: &[P],
    threads: Option<NonZeroUsize>,
    parse: fn(&[u8]) -> Result<T, MailParseError>,
) -> Vec<Result<T, MailParseError>>
where
    P: AsRef<[u8]> + Sync,
    T: Send,
{
    if payloads.is_empty() {
        return Vec::new();
    }

    let available = threads
        .or_else(|| thread::available_parallelism().ok())
        .map_or(1, NonZeroUsize::get);
    // Never more workers than there is work for them to do -- counted both ways.
    // By message, because a worker with no index to claim is a spawn and a join
    // for nothing; and by bytes, because sixteen one-kilobyte messages are
    // sixteen indices and about sixteen microseconds of parsing, which is less
    // than one thread costs to create.
    let total_bytes: usize = payloads.iter().map(|payload| payload.as_ref().len()).sum();
    let by_bytes = (total_bytes / MIN_BYTES_PER_WORKER).max(1);
    let workers = available.min(payloads.len()).min(by_bytes).max(1);

    if workers == 1 {
        return payloads
            .iter()
            .map(|payload| parse(payload.as_ref()))
            .collect();
    }

    let cursor = AtomicUsize::new(0);
    // Each worker claims indices until the batch is exhausted and keeps its own
    // results, so no synchronisation is needed on the output and no `unsafe` is
    // involved.
    let claim = || {
        let mut mine = Vec::new();
        loop {
            let index = cursor.fetch_add(1, Ordering::Relaxed);
            if index >= payloads.len() {
                break;
            }
            mine.push((index, parse(payloads[index].as_ref())));
        }
        mine
    };

    let collected: Vec<Vec<(usize, Result<T, MailParseError>)>> = thread::scope(|scope| {
        // `workers - 1` spawned, because the calling thread runs the same loop
        // rather than blocking on joins. It is already here and already off the
        // GIL, so making it wait was one spawn and one join per call for nothing
        // -- half the thread cost when `workers == 2`.
        let handles: Vec<_> = (1..workers).map(|_| scope.spawn(claim)).collect();

        let mut collected = vec![claim()];
        collected.extend(
            handles
                .into_iter()
                // A worker only panics if the parser does, which is a bug rather
                // than a malformed-input case. Resuming the unwind keeps the
                // behaviour identical to the single-message path, where PyO3 turns a
                // panic into a Python exception instead of losing it. A panic on
                // the calling thread unwinds out of `thread::scope`, which joins
                // the spawned workers first, and lands in the same place.
                .map(|handle| match handle.join() {
                    Ok(results) => results,
                    Err(payload) => std::panic::resume_unwind(payload),
                }),
        );
        collected
    });

    // Restore input order. Slots are filled exactly once, so no gaps.
    let mut ordered: Vec<Option<Result<T, MailParseError>>> =
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
///
/// The second value is `true` when the label was not recognised and the bytes
/// were decoded as us-ascii instead -- a lossy repair, because `decode_ascii`
/// turns every non-ASCII byte into U+FFFD. Reported as a flag rather than by
/// taking the warning collector: this function is called once per body part and
/// gets inlined into that loop, so it stays free of anything that allocates or
/// formats. The caller pushes the warning, out of line.
fn decode_charset(body: &[u8], ctype: &ParsedContentType) -> (String, bool) {
    if let Some(charset) = Charset::for_label(ctype.charset.as_bytes()) {
        (charset.decode(body).0.into_owned(), false)
    } else {
        (decode_ascii(body).into_owned(), true)
    }
}

/// Charset-decode a body, borrowing where the transfer encoding allows it.
///
/// A 7bit/8bit/binary body *is* its raw bytes, so `get_body_raw` copied it into a
/// `Vec` only for `decode_charset` to borrow it straight back. Those arms decode
/// from the slice instead, and the copy is gone.
///
/// Base64 and quoted-printable keep the original route. Handing their decoded
/// `Vec` to `String::from_utf8` instead of lending it to `encoding_rs` looks like
/// it should save a copy, and it does -- but it measured **8% slower** on a 128 KB
/// UTF-8 body, because `encoding_rs` validates with SIMD and `std` does not, and
/// that difference is larger than the copy. Measured, not assumed; do not
/// "optimise" this arm without re-measuring `parse_base64_utf8_text`.
fn decode_body(
    body: &Body<'_>,
    ctype: &ParsedContentType,
) -> Result<(String, bool), MailParseError> {
    match body {
        // Already the bytes; decode straight from the borrowed slice.
        Body::SevenBit(raw) | Body::EightBit(raw) => Ok(decode_charset(raw.get_raw(), ctype)),
        Body::Binary(raw) => Ok(decode_charset(raw.get_raw(), ctype)),
        // The transfer decode allocates, so give that `Vec` away rather than
        // lending it out and copying the result.
        Body::Base64(encoded) | Body::QuotedPrintable(encoded) => {
            Ok(decode_charset(&encoded.get_decoded()?, ctype))
        }
    }
}

/// `ParsedMail::get_body_raw`'s match, against a `Body` the caller already has.
///
/// Verbatim, so an attachment's bytes stay exactly what they were; the only
/// reason it exists here is that `get_body_raw` would call `get_body_encoded`
/// again.
fn body_to_vec(body: &Body<'_>) -> Result<Vec<u8>, MailParseError> {
    match body {
        Body::Base64(body) | Body::QuotedPrintable(body) => body.get_decoded(),
        Body::SevenBit(body) | Body::EightBit(body) => Ok(Vec::<u8>::from(body.get_raw())),
        Body::Binary(body) => Ok(Vec::<u8>::from(body.get_raw())),
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
    // Presence only -- the token itself comes from `kind`. `get_first_header` has
    // the same case-insensitive first-match semantics as `get_first_value` and
    // stops there, where `get_first_value` went on to normalise the value into a
    // `String` that was dropped on the next line.
    part.get_headers().get_first_header("Content-Disposition")?;
    Some(match kind {
        DispositionType::Inline => "inline".to_owned(),
        DispositionType::Attachment => "attachment".to_owned(),
        DispositionType::FormData => "form-data".to_owned(),
        DispositionType::Extension(other) => other.clone(),
    })
}

/// The eight envelope fields every flat mode reads, derived once (#234).
///
/// Three copies of these eleven statements existed, one per flat entry point,
/// and they had begun to drift -- `header_addresses` documented ten call sites
/// when there were fifteen. The envelope is what `strict=True` and the whole
/// warning channel are about, so three derivations of it were three chances for
/// two views of one message to disagree, which is the failure the
/// `parse_agreement` fuzz target was written to catch.
struct Envelope {
    headers: Vec<(String, Vec<String>)>,
    subject: String,
    date: String,
    from_: Option<Address>,
    to: Vec<Address>,
    cc: Vec<Address>,
    bcc: Vec<Address>,
    reply_to: Vec<Address>,
}

/// Read a message's envelope, appending any repair it notices to `warnings`.
///
/// `#[inline]`, and deliberately branch-free: what #100 measured at +47% was
/// threading a *mode* through the parse, a runtime branch in the hot path taken
/// for the benefit of the cold one. This has no mode and no branch -- each caller
/// gets the same straight line of instructions it emitted when it owned a copy,
/// which is the property that makes sharing it free.
///
/// The `Date`-parses check is *not* here. Metadata mode does not make it, on
/// purpose (see `metadata_from_payload`), and a parameter to say so would be
/// exactly the branch this avoids -- so it stays one line at the two callers that
/// want it.
#[inline]
fn envelope(mail: &ParsedMail<'_>, warnings: &mut Vec<Warning>) -> Envelope {
    let headers = collect_headers(mail);

    // Read straight from the parsed headers rather than back out of the map
    // above, so the dedicated fields do not inherit its representation (#28).
    // `get_first_value` is the first occurrence, which is the correct choice for
    // a header that should appear once.
    let subject = mail
        .get_headers()
        .get_first_value("Subject")
        .unwrap_or_default();
    let date = mail
        .get_headers()
        .get_first_value("Date")
        .unwrap_or_default();

    // Address headers are parsed from their first occurrence, like Subject and
    // Date. `From` is a single mailbox in practice, so it is exposed as one
    // value; the first mailbox is taken if a message declares several.
    let from_ = header_addresses(mail, "From", warnings).into_iter().next();
    let to = header_addresses(mail, "To", warnings);
    let cc = header_addresses(mail, "Cc", warnings);
    let bcc = header_addresses(mail, "Bcc", warnings);
    let reply_to = header_addresses(mail, "Reply-To", warnings);

    Envelope {
        headers,
        subject,
        date,
        from_,
        to,
        cc,
        bcc,
        reply_to,
    }
}

/// What every flat mode decides about a part before it looks at the body (#234).
///
/// The *rule*, separate from the identity below it, because the rule applies to
/// every part and the identity is only ever read for an attachment. Bundling the
/// two -- which is the obvious shape -- would derive a `Content-ID` and a
/// disposition token for every `text/plain` body in every message, work no mode
/// does today.
struct PartInfo<'p> {
    mime: &'p str,
    disposition: ParsedContentDisposition,
    /// The RFC 2183 answer to body-vs-attachment (#25).
    is_body: bool,
}

/// Classify one flattened part, or `None` for a `multipart/*` container.
///
/// `multipart/*` nodes are MIME structure, not content: their body is the
/// boundary-delimited concatenation of children already visited, and emitting
/// them produced phantom, filename-less `attachments` entries (#22).
///
/// RFC 2183 decides body-vs-attachment -- not the media type, and not the mere
/// presence of a filename (#25):
///
///   * `Content-Disposition: attachment` means "not for inline display", so a
///     `text/plain` part marked that way is a file whose bytes must not be
///     concatenated into the body.
///   * anything else that is `text/plain` or `text/html` is body text, even when
///     it carries a `Content-Type; name` parameter. A `name` alone previously
///     made the body vanish.
#[inline]
fn classify_part<'p>(part: &'p ParsedMail<'_>) -> Option<PartInfo<'p>> {
    let mime = part.ctype.mimetype.as_str();
    if mime.starts_with("multipart/") {
        return None;
    }

    let disposition = part.get_content_disposition();
    let is_body = disposition.disposition != DispositionType::Attachment
        && matches!(mime, "text/plain" | "text/html");

    Some(PartInfo {
        mime,
        disposition,
        is_body,
    })
}

/// The three fields that say which part this is, in every mode (#234).
///
/// Derived identically by the three flat parsers and by the tree traversal, so it
/// is one function rather than four near-copies -- the drift this would hide is a
/// part that answers to a different name depending on which API asked.
struct PartIdentity {
    filename: String,
    content_id: Option<String>,
    disposition: Option<String>,
}

#[inline]
fn part_identity(part: &ParsedMail<'_>, disposition: &ParsedContentDisposition) -> PartIdentity {
    PartIdentity {
        filename: part_filename(disposition, &part.ctype),
        content_id: part
            .get_headers()
            .get_first_value("Content-ID")
            .map(|raw| normalize_content_id(&raw)),
        disposition: disposition_token(part, &disposition.disposition),
    }
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
    let count = part.headers.len();
    let mut headers: Vec<(String, Vec<String>)> = Vec::with_capacity(count);
    // Keyed by the raw key bytes rather than by the decoded `String`. Latin-1
    // decoding is injective, so byte equality is exactly the `String` equality
    // this map had -- and keys stay case-sensitive, so `Received` and `received`
    // remain separate entries as before. It saves the second owned key per
    // header: `get_key()` allocated once for the map and once for the table.
    let mut positions: HashMap<&[u8], usize> = HashMap::with_capacity(count);

    for header in part.get_headers() {
        match positions.get(header.get_key_raw()).copied() {
            Some(position) => headers[position].1.push(header.get_value()),
            None => {
                positions.insert(header.get_key_raw(), headers.len());
                headers.push((header.get_key(), vec![header.get_value()]));
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
fn encoded_size(body: &Body<'_>) -> usize {
    match body {
        Body::Base64(body) | Body::QuotedPrintable(body) => body.get_raw().len(),
        Body::SevenBit(body) | Body::EightBit(body) => body.get_raw().len(),
        Body::Binary(body) => body.get_raw().len(),
    }
}

/// A non-body part, described but not decoded (#97).
#[derive(Debug)]
pub struct AttachmentMetadata {
    pub mimetype: String,
    pub filename: String,
    pub content_id: Option<String>,
    pub disposition: Option<String>,
    pub encoded_size: usize,
}

/// What a message says about itself, without decoding what it carries (#97).
///
/// Deliberately has no `text_plain`/`text_html`. The issue proposed empty lists,
/// and an empty list is indistinguishable from "this message has no text part" --
/// a triage sweep counting bodyless messages would count every message. Absent
/// attributes fail loudly instead. For structure without decoding, use the tree
/// API (#99).
#[derive(Debug)]
pub struct MailMetadata {
    pub subject: String,
    pub date: String,
    pub from_: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub attachments: Vec<AttachmentMetadata>,
    pub headers: Vec<(String, Vec<String>)>,
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
pub fn parse_email_metadata(payload: &[u8]) -> Result<MailMetadata, MailParseError> {
    if payload.len() > MAX_INPUT_BYTES {
        return Err(MailParseError::Generic(ERR_INPUT_TOO_LARGE));
    }

    // The same repair as the flat path and the tree, so no two views of a message
    // whose header block was never terminated can disagree about it (#150).
    //
    // This entry point was the one the repair missed, because it landed while
    // that work was in flight. Nothing caught it: the corpus test for this mode
    // still excluded the only fixture with the defect. The `parse_agreement` fuzz
    // target found it on its first run, which is the case it was written for --
    // two derivations of one message quietly disagreeing.
    //
    // Split the way `Mail::new` is split, and for the same two reasons: the
    // borrow of the repaired local never leaves this frame, and the body stays in
    // a function of its own. Inlining the repair into the body instead cost the
    // *flat* path 28% and this one 95%, from a scan that reads 1.7 KB of a 767 KB
    // message -- codegen, not work. See CONTRIBUTING.md's Performance section.
    let repaired = repair_missing_separator(payload);
    metadata_from_payload(repaired.as_deref().unwrap_or(payload))
}

#[inline(never)]
fn metadata_from_payload(payload: &[u8]) -> Result<MailMetadata, MailParseError> {
    let mail = parse_mail(payload)?;

    // Metadata mode collects warnings and drops them, which is deliberate rather
    // than an omission (#100). The value of `warnings` is the empty list meaning
    // "nothing was repaired", and this mode never reads a body -- so an empty
    // list here could only ever mean "nothing in the *headers* was repaired".
    // Exposing the same attribute with a weaker guarantee would break the one
    // property it exists to provide, so the channel stays on the mode that can
    // honour it, and `strict=True` is rejected for this one at the boundary. A
    // metadata-specific channel, named for what it can actually see, is a
    // separate decision from this one.
    let mut discarded: Vec<Warning> = Vec::new();
    let Envelope {
        headers,
        subject,
        date,
        from_,
        to,
        cc,
        bcc,
        reply_to,
    } = envelope(&mail, &mut discarded);

    // No `warn_date` here, unlike the two modes that keep their warnings: this
    // one would only drop it, and `parse_date_epoch` is a real `dateparse` call
    // over the header.

    let mut attachments = vec![];

    for part in Mail::extract_mail_parts(mail, 0)? {
        let Some(info) = classify_part(&part) else {
            continue;
        };

        // A body part is skipped entirely here: reporting it with no content and
        // no size would say less than nothing.
        if info.is_body {
            continue;
        }

        let identity = part_identity(&part, &info.disposition);

        attachments.push(AttachmentMetadata {
            mimetype: info.mime.to_string(),
            filename: identity.filename,
            content_id: identity.content_id,
            disposition: identity.disposition,
            encoded_size: encoded_size(&part.get_body_encoded()),
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

/// A non-body part kept encoded, to be decoded on demand (#97).
///
/// `raw` is the part exactly as it sits in the message: its own headers, the
/// separator, and its still-encoded body. That is what makes a later decode
/// reproduce what the full parse produces now, and it is precisely what
/// mailparse's `raw_bytes` is for a subpart -- `parse_mail_recursive` hands each
/// part `&raw_data[ix_part_start..ix_part_end]`, where the start is past the
/// boundary line's newline and the end has the trailing CRLF stripped by
/// `strip_trailing_crlf`, and both the part's header slice and its body slice
/// are taken from inside that same slice. Re-parsing it therefore yields the
/// same body bytes and the same `Content-Transfer-Encoding`, so `get_body_raw`
/// on it is byte-for-byte the full parse's `content`.
///
/// Asserted rather than assumed: over the whole fixture and RFC corpus in
/// `tests/test_lazy_mode.py`, and on arbitrary input by the `parse_agreement`
/// fuzz target.
///
/// The trade is memory for decoding. Base64 is about 1.33x the size of what it
/// encodes, so a retained part costs *more* than the decoded bytes it avoids
/// producing -- right for finding the one PDF in a mailbox, wrong for decoding
/// everything anyway.
/// Where a deferred leaf's encoded bytes live.
///
/// Lazy mode used to copy every deferred part out of the message -- which is the
/// opposite of what deferring is for. On a mailbox sweep looking for one PDF,
/// the copies *are* the cost. The bytes are already in the buffer the caller
/// handed us, so a range into that buffer is enough, provided the buffer outlives
/// the result. That is the contract change: a `LazyMail` now pins its payload.
///
/// `Owned` is the exception rather than the rule, and there are exactly two of
/// them: a leaf inside a decoded `message/rfc822` body, whose bytes were produced
/// by decoding and are in no caller buffer, and a message whose header block had
/// to be repaired (#150), where the parse ran over a rebuilt copy. The repaired
/// copy is returned alongside the result so a range can still index it.
#[derive(Debug)]
pub enum Retained {
    /// A sub-range of the buffer the message was parsed from.
    Range(std::ops::Range<usize>),
    /// An owned copy, for bytes that are in no caller buffer.
    Owned(Vec<u8>),
}

impl Retained {
    /// Retain `part`: as offsets when it is a subslice of `base`, as a copy when
    /// it is not.
    ///
    /// mailparse's `raw_bytes` is a subslice of the buffer given to `parse_mail`
    /// -- it is produced by slicing and never by copying -- so the range arm is
    /// the one that is taken for every part of a message parsed in one piece.
    /// The copy arm exists for the parts that genuinely are not in that buffer:
    /// a leaf below a `message/rfc822` node, whose enclosing bytes had to be
    /// transfer-decoded into a fresh `Vec` before the message inside could be
    /// parsed at all.
    ///
    /// Checked rather than asserted. Computing offsets from a pointer that is not
    /// in `base` would produce a range that indexes out of bounds later, far from
    /// the mistake; the bounds test that avoids it is two comparisons against a
    /// parse that has already walked every byte of the part. The debug assertion
    /// still fires in the tests, so a new caller that expected to borrow and
    /// silently copies is a test failure rather than a performance mystery.
    pub fn of(base: &[u8], part: &[u8]) -> Retained {
        let (base_start, part_start) = (base.as_ptr() as usize, part.as_ptr() as usize);
        let within = part_start >= base_start && part_start + part.len() <= base_start + base.len();
        debug_assert!(
            within,
            "retained bytes are not a subslice of the parsed buffer"
        );
        if within {
            let start = part_start - base_start;
            Retained::Range(start..start + part.len())
        } else {
            Retained::Owned(part.to_vec())
        }
    }

    /// The bytes, given the buffer they were parsed from.
    ///
    /// `base` must be the buffer the parse ran over -- the caller's payload, or
    /// the repaired copy the result carries when one was made. Indexing a
    /// `Range` into anything else is a logic error that would silently return
    /// the wrong bytes, which is why the repaired buffer travels with the result
    /// instead of being dropped.
    pub fn slice<'a>(&'a self, base: &'a [u8]) -> &'a [u8] {
        match self {
            Retained::Range(range) => &base[range.clone()],
            Retained::Owned(bytes) => bytes,
        }
    }

    /// The number of bytes retained, without needing the base buffer.
    pub fn len(&self) -> usize {
        match self {
            Retained::Range(range) => range.len(),
            Retained::Owned(bytes) => bytes.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug)]
pub struct LazyAttachment {
    pub mimetype: String,
    pub filename: String,
    pub content_id: Option<String>,
    pub disposition: Option<String>,
    /// Bytes the part's body occupies before transfer-decoding -- the same value
    /// and the same name as in metadata mode.
    pub encoded_size: usize,
    /// Where the part's encoded bytes are, relative to the buffer the parse ran
    /// over: the caller's payload, or `LazyMail::repaired` when that is set.
    pub raw: Retained,
}

/// A message with its bodies decoded and its attachments deferred (#97).
///
/// Everything except `attachments` is what `Mail` holds, and holds it for the
/// same reasons: lazy mode changes *when* attachment content is materialised and
/// nothing else. `warnings` is therefore the same list the full parse produces,
/// which is what lets `strict=True` mean the same thing in both modes.
#[derive(Debug)]
pub struct LazyMail {
    /// The rebuilt payload, when the header block had to be repaired (#150).
    ///
    /// Returned rather than dropped because the attachments' `Retained::Range`
    /// offsets are relative to whatever buffer the parse ran over, and for a
    /// repaired message that is this copy, not the caller's payload. Dropping it
    /// would leave ranges indexing a buffer that no longer matches.
    pub repaired: Option<Vec<u8>>,
    pub subject: String,
    pub text_plain: Vec<String>,
    pub text_html: Vec<String>,
    pub date: String,
    pub from_: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub attachments: Vec<LazyAttachment>,
    pub headers: Vec<(String, Vec<String>)>,
    pub warnings: Vec<Warning>,
}

/// Decode one retained part, exactly as the full parse would have decoded it.
///
/// A pure function of the bytes -- no cache, no shared state, nothing static.
/// The decode-once cache lives in the binding layer, on the Python object that
/// owns the bytes, so the thread-safety invariant at the top of this module is
/// untouched by lazy mode existing.
///
/// The re-parse is a header scan over one part, which is why the deferral is
/// worth anything: the cost it defers is the transfer-decode and the copy, and
/// the cost it adds is parsing a few hundred bytes of headers again.
pub fn decode_part(raw: &[u8]) -> Result<Vec<u8>, MailParseError> {
    let part = parse_mail(raw)?;
    part.get_body_raw()
}

/// Parse a message, decoding the bodies and deferring the attachments (#97).
///
/// Split into two functions, and duplicating the envelope extraction of
/// `Mail::new`, for the reasons recorded on `parse_email_metadata`: the borrow of
/// the repaired local never leaves this frame, and threading a mode through
/// `Mail::from_payload` instead is what cost the hot path 47% in #100.
pub fn parse_email_lazy(payload: &[u8]) -> Result<LazyMail, MailParseError> {
    if payload.len() > MAX_INPUT_BYTES {
        return Err(MailParseError::Generic(ERR_INPUT_TOO_LARGE));
    }

    // The same repair as the flat path, the tree and metadata mode, so no two
    // views of a message whose header block was never terminated can disagree
    // about it (#150).
    let repaired = repair_missing_separator(payload);
    let mut mail = lazy_from_payload(repaired.as_deref().unwrap_or(payload))?;
    if repaired.is_some() {
        warn_separator(&mut mail.warnings);
    }
    // The ranges in `mail.attachments` index whichever buffer was parsed, so the
    // repaired copy has to travel with the result.
    mail.repaired = repaired;
    Ok(mail)
}

#[inline(never)]
fn lazy_from_payload(payload: &[u8]) -> Result<LazyMail, MailParseError> {
    let mail = parse_mail(payload)?;

    let mut warnings: Vec<Warning> = Vec::new();
    let Envelope {
        headers,
        subject,
        date,
        from_,
        to,
        cc,
        bcc,
        reply_to,
    } = envelope(&mail, &mut warnings);

    if !date.is_empty() && parse_date_epoch(&date).is_none() {
        warn_date(&mut warnings, &date);
    }

    let mut attachments = vec![];
    let mut text_plain = vec![];
    let mut text_html = vec![];

    for part in Mail::extract_mail_parts(mail, 0)? {
        let Some(info) = classify_part(&part) else {
            continue;
        };
        let mime = info.mime;

        // One `get_body_encoded()` per part, threaded to everything below that
        // needs it: the escape check, the encoded size, and the body decode. It
        // re-reads the part's headers to find the transfer encoding, so calling
        // it three times meant three lookups for one answer.
        let body = part.get_body_encoded();

        // Reads the *encoded* bytes and decodes nothing, so this check is as
        // available here as in full mode -- which is why the two modes report
        // the same warnings even for a part whose content is never decoded.
        let lossy_escape = quoted_printable_invalid_escape(&body);

        if !info.is_body {
            if let Some(at) = lossy_escape {
                warn_transfer_decode(&mut warnings, "attachments", attachments.len(), at);
            }

            let identity = part_identity(&part, &info.disposition);

            attachments.push(LazyAttachment {
                mimetype: mime.to_string(),
                filename: identity.filename,
                content_id: identity.content_id,
                disposition: identity.disposition,
                encoded_size: encoded_size(&body),
                // The one copy this mode makes, and what makes it a trade rather
                // than a free win. It is the encoded part, not the decoded one.
                // A range, not a copy: the bytes are already in the buffer
                // being parsed, and copying them is what lazy mode exists to
                // avoid.
                raw: Retained::of(payload, part.raw_bytes),
            });
            continue;
        }

        // Bodies are decoded exactly as the full parse decodes them, including
        // the error a broken transfer encoding raises: this mode defers the
        // attachments and nothing else.
        let (text, fell_back) = decode_body(&body, &part.ctype)?;

        if mime == "text/html" {
            if fell_back {
                let index = text_html.len();
                warn_charset(&mut warnings, "text_html", index, &part.ctype.charset);
            }
            if let Some(at) = lossy_escape {
                warn_transfer_decode(&mut warnings, "text_html", text_html.len(), at);
            }
            text_html.push(text);
        } else {
            // Only `text/plain` reaches here: `is_body` is false for every other
            // media type.
            if fell_back {
                let index = text_plain.len();
                warn_charset(&mut warnings, "text_plain", index, &part.ctype.charset);
            }
            if let Some(at) = lossy_escape {
                warn_transfer_decode(&mut warnings, "text_plain", text_plain.len(), at);
            }
            text_plain.push(text);
        }
    }

    Ok(LazyMail {
        // Filled by the caller, which is the only place that knows whether a
        // repair happened.
        repaired: None,
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
        warnings,
    })
}

/// One node of the MIME tree, as the message actually nests it (#99).
///
/// `Mail` is a flattened projection of this: bodies in one list, attachments in
/// another, containers dropped. Any flattening loses something -- which
/// `text/html` corresponds to which `text/plain` sibling, whether a part was
/// `multipart/alternative` or `multipart/mixed` -- and this keeps it.
///
/// Generic over the body because that is the only thing the three modes disagree
/// about (#237). Everything else a node carries -- its content type, headers,
/// filename, content id, disposition token and children -- is derived the same
/// way in all of them, and used to be derived by two copies of one traversal that
/// a fuzz invariant existed to catch drifting apart. `MimePart` and `TreeNode`
/// are the two bodies that exist; both names are what callers use.
#[derive(Debug)]
pub struct Node<B> {
    pub content_type: String,
    pub headers: Vec<(String, Vec<String>)>,
    pub filename: String,
    pub content_id: Option<String>,
    pub disposition: Option<String>,
    pub is_message: bool,
    pub body: B,
    pub children: Vec<Node<B>>,
}

/// Full mode's tree: `None` is a `multipart/*` container, whose body is just its
/// children with boundaries between them, and `Some` is a leaf's transfer-decoded
/// bytes. `None` means container and nothing else -- see `NodeBody` for why the
/// deferred modes cannot reuse this type.
pub type MimePart = Node<Option<Vec<u8>>>;

/// The deferred modes' tree, whose bodies are described or retained rather than
/// decoded.
pub type TreeNode = Node<NodeBody>;

/// What a mode does with a body, which is all a mode is to this traversal (#237).
///
/// The alternative, and what this replaces, is a second copy of the walk: the
/// depth cap, the `multipart/*` recursion, the `message/rfc822` decode and
/// re-parse, and the six-field node literal, all written twice and kept in step
/// by hand. One of those copies was of the depth cap and the re-parse of an
/// attacker-supplied embedded message, which is not a block to maintain two of.
///
/// Taken by value and `Copy`: a policy is a `Retain` or a unit struct, so passing
/// one costs what passing the old `bool` cost.
trait LeafPolicy: Copy {
    /// What a node of this mode carries where another mode carries something else.
    type Body;

    /// The policy that governs an embedded message's subtree.
    ///
    /// A hook rather than a constant because those bytes are not in the buffer
    /// being parsed: they are a decode of the enclosing body, which is dropped
    /// when the walk leaves the subtree, so a mode that retains offsets has to
    /// stop retaining offsets in there (#239).
    fn inside_embedded(self) -> Self;

    /// A `multipart/*` container.
    fn container(self) -> Self::Body;

    /// A `message/rfc822` node, whose body is `raw` -- already decoded, because
    /// decoding it is what gave this node its child.
    fn embedded(self, part: &ParsedMail<'_>, raw: Vec<u8>) -> Self::Body;

    /// Any other leaf.
    fn leaf(self, part: &ParsedMail<'_>) -> Result<Self::Body, MailParseError>;
}

/// Full mode: every leaf decoded during the walk.
#[derive(Clone, Copy)]
struct Full;

impl LeafPolicy for Full {
    type Body = Option<Vec<u8>>;

    /// Nothing to vary: full mode decodes a leaf wherever it sits.
    fn inside_embedded(self) -> Self {
        Full
    }

    fn container(self) -> Self::Body {
        None
    }

    fn embedded(self, _part: &ParsedMail<'_>, raw: Vec<u8>) -> Self::Body {
        // Published rather than dropped and decoded again: full mode would have
        // decoded this body anyway, and the walk already has it in hand.
        Some(raw)
    }

    fn leaf(self, part: &ParsedMail<'_>) -> Result<Self::Body, MailParseError> {
        Ok(Some(part.get_body_raw()?))
    }
}

/// The deferred modes. `Retain` already says what a leaf keeps of itself, which
/// is exactly what a leaf policy is.
impl LeafPolicy for Retain<'_> {
    type Body = NodeBody;

    fn inside_embedded(self) -> Self {
        match self {
            Retain::Nothing => Retain::Nothing,
            Retain::In(_) | Retain::Copy => Retain::Copy,
        }
    }

    fn container(self) -> Self::Body {
        NodeBody::Container
    }

    fn embedded(self, part: &ParsedMail<'_>, raw: Vec<u8>) -> Self::Body {
        NodeBody::Decoded {
            encoded_size: encoded_size(&part.get_body_encoded()),
            content: raw,
        }
    }

    fn leaf(self, part: &ParsedMail<'_>) -> Result<Self::Body, MailParseError> {
        Ok(NodeBody::Undecoded {
            encoded_size: encoded_size(&part.get_body_encoded()),
            // Offsets into the buffer being parsed, not a copy of it. For the
            // root of a single-part message that range is the whole payload,
            // since mailparse's `raw_bytes` for a root is the message -- see the
            // note in the binding layer. Metadata mode keeps nothing.
            raw: match self {
                Retain::Nothing => None,
                Retain::In(base) => Some(Retained::of(base, part.raw_bytes)),
                Retain::Copy => Some(Retained::Owned(part.raw_bytes.to_vec())),
            },
        })
    }
}

/// Parse a message into its MIME tree, structure intact.
pub fn parse_email_tree(payload: &[u8]) -> Result<MimePart, MailParseError> {
    if payload.len() > MAX_INPUT_BYTES {
        return Err(MailParseError::Generic(ERR_INPUT_TOO_LARGE));
    }

    // The same repair as the flat path, so the two views cannot disagree about a
    // message whose header block was never terminated (#150).
    let repaired = repair_missing_separator(payload);
    build_node(
        &parse_mail(repaired.as_deref().unwrap_or(payload))?,
        0,
        Full,
    )
}

/// What a tree node carries where full mode carries decoded bytes (#202).
///
/// One enum rather than a second and third node struct: the three cases below are
/// what `mode=` varies about a tree node, and everything else about a node --
/// content type, headers, filename, content id, disposition, children -- is the
/// same in all three modes and is derived by the same code. Splitting the struct
/// would have split that derivation too, which is the thing the modes must not
/// disagree about.
///
/// And an enum rather than reusing `MimePart`'s body with a wider meaning.
/// `MimePart`'s body is `Option<Vec<u8>>` and its `None` *means* "this is a
/// container"; a mode where `None` could also mean "not decoded yet" would make
/// the two indistinguishable, which is the silent-wrong-answer shape this crate
/// has rejected twice already (#150, and metadata mode's absent `text_plain`).
/// This says which of the three it is, and `Node<B>` is what lets the two bodies
/// share one traversal without sharing that ambiguity (#237).
#[derive(Debug)]
pub enum NodeBody {
    /// A `multipart/*` container. Its body is its children with boundaries
    /// between them, so it has none of its own -- the same statement full mode
    /// makes by setting `content` to `None`.
    Container,
    /// A leaf's body, not decoded: its size on the wire, and `raw` set to the
    /// part exactly as it sits in the message when the mode intends to decode it
    /// later. `raw` is mailparse's `raw_bytes` for this part, which is what makes
    /// a later `decode_part` reproduce full mode's `content` -- the same
    /// mechanism, and the same claim, as flat lazy mode (#97). Metadata mode
    /// leaves it `None` and keeps only the size.
    ///
    /// `Retained` rather than `Vec<u8>`: for a leaf of the message itself those
    /// bytes are already in the buffer being parsed and are kept as offsets into
    /// it (#239). A leaf below a `message/rfc822` node is the exception and is
    /// copied, because the buffer it sits in was produced by decoding and is
    /// dropped when the walk leaves that subtree.
    Undecoded {
        encoded_size: usize,
        raw: Option<Retained>,
    },
    /// Decoded during the walk, because the structure below it needed it.
    ///
    /// Only `message/rfc822`. Its body *is* the embedded message, so parsing it
    /// is what gives the node children at all -- and deferring the decode would
    /// mean deferring the structure, which is the one thing every mode of a tree
    /// API has to deliver eagerly. The bytes are already in hand by then, so they
    /// are published rather than dropped and decoded again.
    Decoded {
        encoded_size: usize,
        content: Vec<u8>,
    },
}

impl NodeBody {
    /// Bytes this body occupies before transfer-decoding, or `None` for a
    /// container -- which has no body of its own.
    pub fn encoded_size(&self) -> Option<usize> {
        match self {
            NodeBody::Container => None,
            NodeBody::Undecoded { encoded_size, .. } | NodeBody::Decoded { encoded_size, .. } => {
                Some(*encoded_size)
            }
        }
    }
}

/// Parse a message into its MIME tree without decoding the leaves (#202).
///
/// `defer` picks what a leaf keeps: a copy of the part, to decode on demand
/// (`mode="lazy"`), or only the size its body occupies on the wire
/// (`mode="metadata"`). The traversal, the classification and the caps are the
/// same either way, which is what keeps the two modes agreeing with each other
/// and with full mode about the shape of a message.
///
/// Unlike flat metadata mode this can still raise a decode failure, for one
/// reason: a `message/rfc822` body has to be decoded before the message inside
/// it can be parsed, and a tree that dropped those children in one mode would
/// not be the same tree. Nothing else is decoded.
pub fn parse_tree_deferred(payload: &[u8], defer: bool) -> Result<DeferredTree, MailParseError> {
    if payload.len() > MAX_INPUT_BYTES {
        return Err(MailParseError::Generic(ERR_INPUT_TOO_LARGE));
    }

    // The same repair as every other entry point, so no two views of a message
    // whose header block was never terminated can disagree about it (#150).
    let repaired = repair_missing_separator(payload);
    let base = repaired.as_deref().unwrap_or(payload);
    let retain = if defer {
        Retain::In(base)
    } else {
        Retain::Nothing
    };
    let root = build_node(&parse_mail(base)?, 0, retain)?;
    Ok(DeferredTree { root, repaired })
}

/// What a leaf of one walk keeps of itself.
///
/// Three states, not two, and that is the point: "keep nothing" and "keep a copy
/// because there is no buffer to point into" are different answers, and a
/// parameter that could only say borrow-or-not answered the second with the first
/// -- which drops the bytes of every leaf inside an embedded message and turns
/// lazy mode into metadata mode for that subtree.
#[derive(Clone, Copy)]
enum Retain<'a> {
    /// Nothing at all: metadata mode, which keeps sizes and no bytes.
    Nothing,
    /// Offsets into this buffer, which the caller of the parse keeps alive.
    In(&'a [u8]),
    /// A copy, for a subtree parsed out of a buffer this walk owns and drops.
    Copy,
}

/// A tree whose leaves are offsets, and the buffer those offsets index when it
/// is not the caller's (#239).
///
/// The pair travels together for the reason recorded on `LazyMail::repaired`: a
/// range is only meaningful next to the buffer the parse ran over, and for a
/// message whose header block had to be repaired that buffer is the rebuilt copy.
#[derive(Debug)]
pub struct DeferredTree {
    pub root: TreeNode,
    pub repaired: Option<Vec<u8>>,
}

/// The one MIME-tree traversal, in every mode (#237).
///
/// `#[inline(never)]`: it recurses, it returns a large struct, and no hot path
/// reaches it. Two instantiations, `Full` and `Retain`, which is what the two
/// hand-written copies this replaced cost the linker -- the collapse is a
/// maintenance change and is not meant to move any benchmark.
///
/// `policy` decides what a body is and nothing else. Everything below this line
/// is what every mode agrees about, which is the point: the depth cap, the
/// recursion over `multipart/*`, the decode-and-re-parse of an embedded message,
/// and how a node's identity is derived from its headers.
#[inline(never)]
fn build_node<P: LeafPolicy>(
    part: &ParsedMail<'_>,
    depth: usize,
    policy: P,
) -> Result<Node<P::Body>, MailParseError> {
    if depth >= MAX_MIME_DEPTH {
        return Err(MailParseError::Generic(ERR_MIME_DEPTH));
    }

    let mime = part.ctype.mimetype.as_str();

    let (body, children) = if mime.starts_with("multipart/") {
        // A container's body is the boundary-delimited concatenation of the
        // children below it, so reporting it as a body would report the same
        // bytes twice.
        let children = part
            .subparts
            .iter()
            .map(|child| build_node(child, depth + 1, policy))
            .collect::<Result<Vec<_>, _>>()?;
        (policy.container(), children)
    } else if mime == "message/rfc822" {
        // An embedded message -- a bounce or a forward, which abuse pipelines are
        // made of. mailparse hands it over as an opaque leaf; parsing it is the
        // difference between "there is a message in here" and being able to read
        // its headers. Every mode does this, including the ones that decode
        // nothing else, because the body *is* the child.
        //
        // The nesting counts against the same depth cap, so an onion of forwards
        // cannot recurse further than a multipart tree can. That this is now one
        // copy rather than two is most of why this issue was worth doing: it is
        // the cap, and a re-parse of attacker-supplied bytes, in one place.
        let raw = part.get_body_raw()?;
        let inner = {
            let repaired = repair_missing_separator(&raw);
            let parsed = parse_mail(repaired.as_deref().unwrap_or(raw.as_slice()))?;
            build_node(&parsed, depth + 1, policy.inside_embedded())?
        };
        (policy.embedded(part, raw), vec![inner])
    } else {
        (policy.leaf(part)?, Vec::new())
    };

    // The same three fields the flat modes derive, from the same helper (#234):
    // a part must not answer to a different name depending on which API asked.
    // Derived for a container too, which the flat modes never see -- they drop
    // containers, and the tree is the API that keeps them.
    let identity = part_identity(part, &part.get_content_disposition());

    Ok(Node {
        content_type: mime.to_string(),
        headers: collect_headers(part),
        filename: identity.filename,
        content_id: identity.content_id,
        disposition: identity.disposition,
        is_message: mime == "message/rfc822",
        body,
        children,
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
pub fn parse_date_epoch(date: &str) -> Option<i64> {
    let upper = date.to_uppercase();
    if !MONTH_TOKENS.iter().any(|month| upper.contains(month)) {
        return None;
    }
    dateparse(date).ok()
}

/// One mailbox from an address header.
#[derive(Debug, Clone)]
pub struct Address {
    pub display_name: Option<String>,
    pub address: String,
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
/// That silence is what `warnings` ends: the dropped mailboxes are recorded as
/// an `address-unparseable` warning (#100). An *absent* header is not a repair
/// and is not reported.
fn parse_addresses(
    header: Option<&MailHeader<'_>>,
    name: &'static str,
    warnings: &mut Vec<Warning>,
) -> Vec<Address> {
    let Some(header) = header else {
        return Vec::new();
    };
    let Ok(parsed) = addrparse_header(header) else {
        warn_address(warnings, name);
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

/// Parse one named address header from a message's first occurrence of it.
///
/// A thin wrapper so its call sites -- all five of them, in `envelope` -- stay
/// one short line each: the header lookup has to happen inside the same
/// expression as the parse, because
/// `get_first_header` borrows the temporary `Headers` that `get_headers()`
/// builds, so the two cannot be split across statements.
fn header_addresses(
    mail: &ParsedMail<'_>,
    name: &'static str,
    warnings: &mut Vec<Warning>,
) -> Vec<Address> {
    parse_addresses(mail.get_headers().get_first_header(name), name, warnings)
}

#[derive(Debug)]
pub struct Mail {
    pub subject: String,
    pub text_plain: Vec<String>,
    pub text_html: Vec<String>,
    pub date: String,
    pub from_: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub attachments: Vec<Attachment>,
    pub headers: Vec<(String, Vec<String>)>,
    /// Every lossy repair this parse made, in the order it made them. Empty for
    /// a pristine parse, which is the overwhelmingly common case and the one
    /// that must stay free -- see [`Warning`].
    pub warnings: Vec<Warning>,
}

#[derive(Debug)]
pub struct Attachment {
    pub mimetype: String,
    pub content: Vec<u8>,
    pub filename: String,
    pub content_id: Option<String>,
    pub disposition: Option<String>,
}

impl Mail {
    /// Parse one message.
    ///
    /// Two steps, so that what mailparse sees is normalised first: a message
    /// missing its header/body separator is parsed from a repaired copy of the
    /// payload (#150). `from_payload` reads whatever buffer it is handed and
    /// returns fully owned data, so that copy can be a local here.
    pub fn new(payload: &[u8]) -> Result<Self, MailParseError> {
        // Measured against the payload as received: a repair adds one byte, and
        // no message should become oversized by being repaired.
        if payload.len() > MAX_INPUT_BYTES {
            return Err(MailParseError::Generic(ERR_INPUT_TOO_LARGE));
        }

        // The repair is what saves the body; the warning is what makes the repair
        // observable, which is the half #150 left to #100.
        //
        // One call site, deliberately. Branching on `repaired` around two calls
        // to `from_payload` would read better and is exactly what #187 measured
        // the cost of: `from_payload` carries no `inline(never)`, so a second
        // call site is a second chance to inline the whole parse body, and
        // duplicating it there cost the flat path 28%. The `is_some` below reads
        // a local after the borrow of it has ended.
        let repaired = repair_missing_separator(payload);
        let mut mail = Mail::from_payload(repaired.as_deref().unwrap_or(payload))?;
        if repaired.is_some() {
            warn_separator(&mut mail.warnings);
        }
        Ok(mail)
    }
}

impl<'a> Mail {
    fn from_payload(payload: &'a [u8]) -> Result<Self, MailParseError> {
        let mail = parse_mail(payload)?;

        // `Vec::new` does not allocate, so a parse that repairs nothing -- which
        // is nearly all of them -- pays three words of stack for this and
        // nothing else.
        let mut warnings: Vec<Warning> = Vec::new();

        let Envelope {
            headers,
            subject,
            date,
            from_,
            to,
            cc,
            bcc,
            reply_to,
        } = envelope(&mail, &mut warnings);

        // A Date that does not parse loses nothing -- `date` keeps the raw
        // string -- but `date_parsed` goes quietly to `None`, and "quietly" is
        // what this channel exists to fix. Checked here rather than in the
        // `date_parsed` getter because the warning list has to be complete when
        // the parse returns; the cost is one `dateparse` over a ~30-byte header,
        // and only for messages that carry a Date at all.
        if !date.is_empty() && parse_date_epoch(&date).is_none() {
            warn_date(&mut warnings, &date);
        }

        let mut attachments = vec![];
        let mut text_plain = vec![];
        let mut text_html = vec![];

        for part in Self::extract_mail_parts(mail, 0)? {
            // The `multipart/*` skip and the RFC 2183 body-vs-attachment rule,
            // shared with the other two flat modes so they cannot disagree about
            // what a part is (#234).
            let Some(info) = classify_part(&part) else {
                continue;
            };
            let mime = info.mime;

            // Undo the Content-Transfer-Encoding (e.g. base64/quoted-printable)
            // exactly once. `?` propagates a broken transfer encoding instead of
            // swallowing it with `unwrap_or_default()`, which would silently turn
            // corruption into an empty body; the PyO3 layer surfaces the error to
            // Python as `ParseError`.
            // One `get_body_encoded()` per part, threaded to everything below
            // that needs it. It re-reads the part's headers to find the transfer
            // encoding, so calling it once for the escape check and again for the
            // decode meant two lookups for one answer.
            let body = part.get_body_encoded();

            // Checked once per part, reported below with the index the part
            // actually lands at, so `part_path` locates it in the result.
            let lossy_escape = quoted_printable_invalid_escape(&body);

            if !info.is_body {
                if let Some(at) = lossy_escape {
                    warn_transfer_decode(&mut warnings, "attachments", attachments.len(), at);
                }

                let identity = part_identity(&part, &info.disposition);

                attachments.push(Attachment {
                    mimetype: mime.to_string(),
                    // Byte-identical to `get_body_raw()`: same match, same arms.
                    content: body_to_vec(&body)?,
                    filename: identity.filename,
                    content_id: identity.content_id,
                    disposition: identity.disposition,
                });
            } else if mime == "text/html" {
                // For text parts, build the Python-facing string from the bytes
                // just decoded rather than calling `get_body()`, which would re-run
                // the identical transfer decode. `decode_charset` performs only the
                // charset step, so the result matches mailparse's `get_body` output
                // byte-for-byte (see `decode_charset`).
                let (text, fell_back) = decode_body(&body, &part.ctype)?;
                if fell_back {
                    let index = text_html.len();
                    warn_charset(&mut warnings, "text_html", index, &part.ctype.charset);
                }
                if let Some(at) = lossy_escape {
                    warn_transfer_decode(&mut warnings, "text_html", text_html.len(), at);
                }
                text_html.push(text);
            } else {
                // Only `text/plain` reaches here: `is_body` is false for every
                // other media type.
                let (text, fell_back) = decode_body(&body, &part.ctype)?;
                if fell_back {
                    let index = text_plain.len();
                    warn_charset(&mut warnings, "text_plain", index, &part.ctype.charset);
                }
                if let Some(at) = lossy_escape {
                    warn_transfer_decode(&mut warnings, "text_plain", text_plain.len(), at);
                }
                text_plain.push(text);
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
            warnings,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The core's first Rust tests (#236). Until this crate existed there was no
    /// way to write one: the module was `#[path]`-included into a `cdylib` that
    /// cannot be linked as a Rust dependency, so every assertion about parsing
    /// had to go through Python.
    ///
    /// These deliberately test what is awkward to reach from Python: the DoS
    /// caps, which need a 100 MB payload to trip; the warning machinery, whose
    /// ordering is an internal contract; and the header collector, which Python
    /// only ever sees through a dict.
    const SIMPLE: &[u8] = b"Subject: hi\r\nFrom: a@example.com\r\n\r\nbody\r\n";

    /// The fixtures for the retention tests below. `#[test]` builds run in debug,
    /// where `Retained::of`'s assertion is live -- so a leaf that stopped being a
    /// subslice of the parsed buffer and started being copied fails here rather
    /// than becoming a quiet performance regression.
    const WITH_ATTACHMENT: &[u8] = b"Subject: has one\r\n\
        Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n\
        --b\r\nContent-Type: text/plain\r\n\r\nhello\r\n\
        --b\r\nContent-Type: application/pdf\r\n\
        Content-Disposition: attachment; filename=doc.pdf\r\n\
        Content-Transfer-Encoding: base64\r\n\r\ncGRmIGJ5dGVz\r\n--b--\r\n";

    /// The same message with the blank line after the header block removed, which
    /// is what makes the parse run over a rebuilt copy (#150).
    const UNTERMINATED: &[u8] = b"Subject: has one\r\n\
        Content-Type: multipart/mixed; boundary=\"b\"\r\n\
        this line has no colon\r\n\
        --b\r\nContent-Type: text/plain\r\n\r\nhello\r\n\
        --b\r\nContent-Type: application/pdf\r\n\
        Content-Disposition: attachment; filename=doc.pdf\r\n\
        Content-Transfer-Encoding: base64\r\n\r\ncGRmIGJ5dGVz\r\n--b--\r\n";

    /// One multipart container and three leaves: a body, the same media type
    /// marked as an attachment, and a non-text part.
    const FOUR_PARTS: &[u8] = b"Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n\
        --b\r\nContent-Type: text/plain\r\n\r\nbody text\r\n\
        --b\r\nContent-Type: text/plain\r\n\
        Content-Disposition: attachment; filename=note.txt\r\n\r\nattached\r\n\
        --b\r\nContent-Type: application/pdf\r\n\
        Content-ID: <cid-1>\r\n\r\npdf\r\n--b--\r\n";

    #[test]
    fn classify_part_applies_the_rfc_2183_rule_directly() {
        // The rule had three end-to-end tests, one per flat mode, and no direct
        // one -- which is backwards for the rule that is the reason those modes
        // agree. Now that they share it, it gets its own (#234).
        let mail = parse_mail(FOUR_PARTS).expect("the fixture must parse");

        // Structure, not content (#22): a container classifies as no part at all.
        assert!(
            classify_part(&mail).is_none(),
            "a multipart/* part must not classify as a part"
        );

        let parts: Vec<PartInfo<'_>> = mail
            .subparts
            .iter()
            .map(|part| classify_part(part).expect("a leaf must classify"))
            .collect();

        assert_eq!(
            parts.iter().map(|info| info.mime).collect::<Vec<_>>(),
            vec!["text/plain", "text/plain", "application/pdf"],
        );
        // The middle one is the whole point: same media type as the first, and an
        // attachment because RFC 2183 says so, not because of its type or a
        // filename.
        assert_eq!(
            parts.iter().map(|info| info.is_body).collect::<Vec<_>>(),
            vec![true, false, false],
        );
    }

    #[test]
    fn part_identity_reads_the_name_the_part_answers_to() {
        let mail = parse_mail(FOUR_PARTS).expect("the fixture must parse");
        let identity: Vec<PartIdentity> = mail
            .subparts
            .iter()
            .map(|part| part_identity(part, &part.get_content_disposition()))
            .collect();

        assert_eq!(
            identity
                .iter()
                .map(|id| id.filename.as_str())
                .collect::<Vec<_>>(),
            vec!["", "note.txt", ""],
        );
        // Angle brackets stripped, so a `cid:` URL is a plain lookup (RFC 2392).
        assert_eq!(
            identity
                .iter()
                .map(|id| id.content_id.as_deref())
                .collect::<Vec<_>>(),
            vec![None, None, Some("cid-1")],
        );
        // `None` where the part declares no Content-Disposition at all, which is
        // a different statement from "inline".
        assert_eq!(
            identity
                .iter()
                .map(|id| id.disposition.as_deref())
                .collect::<Vec<_>>(),
            vec![None, Some("attachment"), None],
        );
    }

    #[test]
    fn a_deferred_attachment_is_a_range_and_not_a_copy() {
        let mail = parse_email_lazy(WITH_ATTACHMENT).expect("the fixture must parse");
        let attachment = &mail.attachments[0];

        assert!(
            matches!(attachment.raw, Retained::Range(_)),
            "a leaf of a message parsed in one piece must be retained as offsets, \
             not copied: {:?}",
            attachment.raw
        );
        assert!(mail.repaired.is_none(), "this fixture needs no repair");
        assert_eq!(
            decode_part(attachment.raw.slice(WITH_ATTACHMENT)).expect("decodes"),
            b"pdf bytes",
        );
    }

    #[test]
    fn a_repaired_message_retains_ranges_into_the_rebuilt_buffer() {
        let mail = parse_email_lazy(UNTERMINATED).expect("the fixture must parse");
        let repaired = mail
            .repaired
            .as_deref()
            .expect("the header block was unterminated");
        let attachment = &mail.attachments[0];

        // The offsets index the rebuild, which is one byte longer than the
        // payload. Resolving them against the payload instead is exactly the
        // off-by-one this pairing exists to prevent, so assert the right buffer
        // gives the right bytes rather than merely that some buffer does.
        assert_eq!(repaired.len(), UNTERMINATED.len() + 1);
        assert_eq!(
            decode_part(attachment.raw.slice(repaired)).expect("decodes"),
            b"pdf bytes",
        );
    }

    /// Every per-node field except the body, in walk order.
    ///
    /// Generic over the body for the same reason `Node` is: the claim under test
    /// is that the modes differ in the body and in nothing else, and a renderer
    /// that could only read one of them could not state it.
    fn canonical<B>(node: &Node<B>, out: &mut Vec<String>) {
        out.push(format!(
            "{}|{}|{:?}|{:?}|{}|{:?}|{}",
            node.content_type,
            node.filename,
            node.content_id,
            node.disposition,
            node.is_message,
            node.headers,
            node.children.len(),
        ));
        for child in &node.children {
            canonical(child, out);
        }
    }

    #[test]
    fn every_mode_builds_the_same_tree_around_an_embedded_message() {
        // The in-tree twin of `parse_agreement`'s invariant 8 (#237). The fuzz
        // target has asserted this on arbitrary input since #202, but it is the
        // reason the traversal may be shared at all, so it should also be a test
        // that runs on every `cargo test` rather than only under a nightly
        // fuzzer. An embedded message is the fixture because it exercises the one
        // arm where the modes diverge on more than the leaf: the policy changes
        // for the subtree.
        let mut payload = b"Content-Type: message/rfc822\r\n\r\n".to_vec();
        payload.extend_from_slice(WITH_ATTACHMENT);

        let full = parse_email_tree(&payload).expect("the fixture must parse");
        let described = parse_tree_deferred(&payload, false).expect("the fixture must parse");
        let deferred = parse_tree_deferred(&payload, true).expect("the fixture must parse");

        let (mut a, mut b, mut c) = (Vec::new(), Vec::new(), Vec::new());
        canonical(&full, &mut a);
        canonical(&described.root, &mut b);
        canonical(&deferred.root, &mut c);

        assert!(
            a.len() >= 4,
            "the fixture must nest deeply enough to be worth comparing, got {a:?}"
        );
        assert_eq!(a, b, "the metadata tree's shape disagrees with full mode");
        assert_eq!(a, c, "the lazy tree's shape disagrees with full mode");

        // And the bodies line up where they are comparable: a node has a body of
        // its own in every mode or in none, which is the same question as
        // `encoded_size.is_some()`.
        let mut bodies = Vec::new();
        full_bodies(&full, &mut bodies);
        let mut sizes = Vec::new();
        node_sizes(&described.root, &mut sizes);
        assert_eq!(
            bodies.iter().map(Option::is_some).collect::<Vec<_>>(),
            sizes.iter().map(Option::is_some).collect::<Vec<_>>(),
            "a node reports a body in one mode and not the other"
        );
    }

    fn full_bodies(node: &MimePart, out: &mut Vec<Option<usize>>) {
        out.push(node.body.as_ref().map(Vec::len));
        for child in &node.children {
            full_bodies(child, out);
        }
    }

    fn node_sizes(node: &TreeNode, out: &mut Vec<Option<usize>>) {
        out.push(node.body.encoded_size());
        for child in &node.children {
            node_sizes(child, out);
        }
    }

    #[test]
    fn a_leaf_below_an_embedded_message_is_copied() {
        // The one case that cannot borrow: the bytes were produced by decoding
        // the enclosing body, and that buffer is dropped when the walk leaves the
        // subtree. Asserted so the copy stays deliberate.
        let mut payload = b"Content-Type: message/rfc822\r\n\r\n".to_vec();
        payload.extend_from_slice(WITH_ATTACHMENT);
        let tree = parse_tree_deferred(&payload, true).expect("the fixture must parse");

        let root = match &tree.root.body {
            NodeBody::Decoded { .. } => &tree.root,
            other => panic!("the root should have been decoded to reach its child: {other:?}"),
        };
        let leaves = collect_undecoded(&root.children[0]);

        assert!(!leaves.is_empty(), "the embedded message must have leaves");
        for leaf in leaves {
            assert!(
                matches!(leaf, Retained::Owned(_)),
                "a leaf below an embedded message has no buffer to borrow: {leaf:?}"
            );
        }
    }

    fn collect_undecoded(node: &TreeNode) -> Vec<&Retained> {
        let mut out = Vec::new();
        if let NodeBody::Undecoded { raw: Some(raw), .. } = &node.body {
            out.push(raw);
        }
        for child in &node.children {
            out.extend(collect_undecoded(child));
        }
        out
    }

    #[test]
    fn a_plain_message_parses_with_no_warnings() {
        let mail = parse_email(SIMPLE).expect("a well-formed message must parse");

        assert_eq!(mail.subject, "hi");
        assert_eq!(mail.text_plain.len(), 1);
        assert!(mail.text_plain[0].starts_with("body"));
        assert!(
            mail.warnings.is_empty(),
            "an unrepaired parse must report nothing: {:?}",
            mail.warnings
        );
    }

    #[test]
    fn an_oversized_payload_is_rejected_by_the_cap_not_by_parsing() {
        // The cap is the reason this is cheap to test here and expensive from
        // Python: no 100 MB object has to cross the FFI boundary.
        let payload = vec![b'x'; MAX_INPUT_BYTES + 1];

        let error = parse_email(&payload).expect_err("over the cap must be refused");

        assert!(
            error.to_string().contains(ERR_INPUT_TOO_LARGE),
            "the cap must be named in the error, not just any failure: {error}"
        );
    }

    // The MIME depth cap is deliberately NOT tested here. `tests/test_dos_limits.py`
    // already covers it end to end, and building a genuinely 300-level multipart
    // in a Rust string literal turned out to be easy to get subtly wrong -- two
    // attempts produced payloads the parser flattened, so the test "passed the
    // cap" by never reaching it. A test that can silently stop testing its
    // subject is worse than the coverage it duplicates.

    #[test]
    fn headers_keep_every_value_in_first_appearance_order() {
        let raw = b"Received: one\r\nSubject: s\r\nReceived: two\r\nX-A: a\r\n\r\nbody\r\n";

        let mail = parse_email(raw).expect("parses");
        let names: Vec<&str> = mail.headers.iter().map(|(name, _)| name.as_str()).collect();

        // Wire order of first appearance, not sorted and not de-duplicated into
        // the last value -- both of which a HashMap did before #157.
        assert_eq!(names, ["Received", "Subject", "X-A"]);
        let received = &mail
            .headers
            .iter()
            .find(|(n, _)| n == "Received")
            .unwrap()
            .1;
        assert_eq!(received, &["one", "two"]);
    }

    #[test]
    fn an_unknown_charset_is_a_reported_repair_not_a_failure() {
        let raw = b"Subject: s\r\nContent-Type: text/plain; charset=not-a-charset\r\n\r\nbody\r\n";

        let mail = parse_email(raw).expect("an unknown charset is repaired, not fatal");

        assert!(
            !mail.warnings.is_empty(),
            "falling back to us-ascii is lossy and must be reported"
        );
    }

    #[test]
    fn a_broken_transfer_encoding_is_an_error_not_an_empty_body() {
        // The #24 contract: a failed transfer decode must propagate rather than
        // being swallowed into an empty body.
        let raw = b"Subject: s\r\nContent-Transfer-Encoding: base64\r\n\r\n!!!!not base64!!!!\r\n";

        assert!(
            parse_email(raw).is_err(),
            "corruption must surface, not decode to nothing"
        );
    }

    #[test]
    fn parse_many_preserves_input_order_and_reports_per_slot() {
        let good = SIMPLE.to_vec();
        let bad = b"Subject: s\r\nContent-Transfer-Encoding: base64\r\n\r\n!!!!\r\n".to_vec();
        let payloads = [good.clone(), bad, good];

        let results = parse_many(&payloads, None);

        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(results[1].is_err(), "a bad slot fails on its own");
        assert!(
            results[2].is_ok(),
            "and does not take its neighbours with it"
        );
    }

    #[test]
    fn metadata_mode_agrees_with_the_full_parse_on_the_envelope() {
        let full = parse_email(SIMPLE).expect("parses");
        let metadata = parse_email_metadata(SIMPLE).expect("parses");

        assert_eq!(full.subject, metadata.subject);
        assert_eq!(full.headers, metadata.headers);
    }
}
