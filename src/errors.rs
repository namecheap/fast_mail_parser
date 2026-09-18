//! The exception hierarchy, the mapping onto it, and the panic backstop (#233).
//!
//! One place for every way this extension can fail, because which subtype a
//! failure gets is part of the API: `except ParseError` keeps catching
//! everything, and a caller that cares can tell a header-section failure from a
//! hostile structure from one broken part.

use pyo3::prelude::*;
use pyo3::{create_exception, exceptions};

use fast_mail_parser_core::MailParseError;

use crate::mail_parser;

use crate::flat::ParseWarning;

create_exception!(fast_mail_parser, ParseError, exceptions::PyException);

// Subtypes of ParseError, so an existing `except ParseError` keeps catching
// everything while callers that care can distinguish the failure. The
// distinction is actionable: a header-section failure usually means the input is
// not an email at all, a structure failure means it is hostile or truncated, and
// a decode failure means one part's transfer encoding is broken while the rest
// of the message may still be worth looking at.
create_exception!(
    fast_mail_parser,
    HeaderParseError,
    ParseError,
    "The header section could not be parsed."
);
create_exception!(
    fast_mail_parser,
    MimeStructureError,
    ParseError,
    "The MIME structure is malformed, or a resource cap was exceeded."
);
create_exception!(
    fast_mail_parser,
    DecodeError,
    ParseError,
    "A part's Content-Transfer-Encoding could not be decoded."
);

/// Classify a parse failure and build the matching Python exception.
///
/// Classification is by `MailParseError` variant, which is exact rather than a
/// heuristic: in mailparse 0.16, transfer-decoding (`body.rs`) only ever yields
/// the three decode variants, and `Generic` is produced solely by the
/// header-parsing paths -- plus the two caps this crate originates itself, which
/// are matched by their named constants.
///
/// Deliberately done here rather than by threading a typed error through the
/// parser: an earlier attempt at that widened the `Result` carried by the
/// per-part loop and the recursive traversal and cost ~30% throughput, which the
/// benchmark gate caught. The classification is cold, so it belongs on the cold
/// side of the boundary.
pub(crate) fn to_py_err(error: MailParseError) -> PyErr {
    let message = format!("Message parsing error: {error}");
    match error {
        MailParseError::Base64DecodeError(_)
        | MailParseError::QuotedPrintableDecodeError(_)
        | MailParseError::EncodingError(_) => DecodeError::new_err(message),
        MailParseError::Generic(detail)
            if detail == mail_parser::ERR_INPUT_TOO_LARGE
                || detail == mail_parser::ERR_MIME_DEPTH =>
        {
            MimeStructureError::new_err(message)
        }
        MailParseError::Generic(_) => HeaderParseError::new_err(message),
    }
}

/// Build the exception `strict=True` promises for a lossy parse (#100).
///
/// Strict mode is implemented as a check *after* the parse rather than a flag
/// threaded into it. Two reasons. The core never learns about strictness, so the
/// per-part loop keeps the shape #135 measured -- a mode flag tested inside it
/// would be a branch every message pays for a setting almost nobody sets. And
/// the exception is the same one either way: the mapping below is total over the
/// kinds the core emits, so "raise instead of warn" and "warn, then raise"
/// differ only in that the second finishes the parse first and can therefore
/// report how many repairs there were.
///
/// The mapping reuses the #135 hierarchy rather than adding types: a dropped
/// address list is a header-level failure, a resynced header block is a
/// structural one, and a charset fallback or an unreadable date is a value that
/// could not be decoded.
///
/// `#[cold]` and out of line for the same reason as the `warn_*` helpers in the
/// core: it formats, and it must not be inlined into a parse.
#[cold]
#[inline(never)]
pub(crate) fn strict_rejection(warnings: &[ParseWarning]) -> PyErr {
    // Non-empty by construction: the callers check before calling.
    let warning = &warnings[0];
    let location = if warning.part_path.is_empty() {
        "the message".to_owned()
    } else {
        format!("part {}", warning.part_path)
    };
    let message = format!(
        "strict mode rejected a lossy parse: {} on {}: {} \
         ({} warning(s) recorded; parse without strict=True to read them all)",
        warning.kind,
        location,
        warning.detail,
        warnings.len()
    );
    match warning.kind.as_str() {
        mail_parser::KIND_ADDRESS_UNPARSEABLE => HeaderParseError::new_err(message),
        // Structure rather than header parsing: what the defect breaks is the
        // boundary between the header block and the MIME body, and the message
        // parses fine once that is restored -- so `HeaderParseError`, documented
        // as "usually the input is not an email at all", would say the wrong
        // thing about it.
        mail_parser::KIND_UNTERMINATED_HEADERS => MimeStructureError::new_err(message),
        mail_parser::KIND_CHARSET_FALLBACK
        | mail_parser::KIND_DATE_UNPARSEABLE
        | mail_parser::KIND_TRANSFER_DECODE_LOSSY => DecodeError::new_err(message),
        // A kind added to the core without a row above still fails strict mode,
        // just at the base of the hierarchy. Failing open would break the only
        // promise strict mode makes, which is that nothing lossy gets through.
        _ => ParseError::new_err(message),
    }
}

/// Extract a readable message from a panic payload.
///
/// `panic!` with a literal yields a `&str`; with a format string, a `String`.
/// Anything else is a `panic_any` of some other type, which this crate never
/// does but a dependency could.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<&str>() {
        message
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.as_str()
    } else {
        "unknown panic payload"
    }
}

/// Run a parse and convert a panic into a `ParseError` (#102).
///
/// PyO3 already catches panics at the boundary, so one does not abort the
/// process the way #102 assumed -- it raises `pyo3_runtime.PanicException`. The
/// operational risk is real anyway, for a different reason: `PanicException`
/// derives from `BaseException`, so the `except Exception` in a mail pipeline
/// does not catch it, and a single crafted message takes the worker down.
///
/// A parser fed attacker-controlled bytes should fail like a parser. Raising
/// `ParseError` puts a panic in the same `except` clause as every other
/// unparseable message, and keeping the payload in the message means the bug
/// stays diagnosable rather than swallowed -- the default panic hook has also
/// already written the panic and its location to stderr by this point.
///
/// This is a backstop, not a licence: a panic reaching here is a bug in this
/// crate, and the error says so.
///
/// `inline(always)` is load-bearing, not decoration. This is generic, so every
/// entry point instantiates it, and once there were three the instantiation
/// wrapping `parse_email` stopped being inlined -- which turns the parse body
/// into an opaque call behind unwind edges and cost 24% on large messages while
/// the code responsible was never executed (#99).
#[inline(always)]
pub(crate) fn catch_panics<T>(operation: impl FnOnce() -> PyResult<T>) -> PyResult<T> {
    // `AssertUnwindSafe`: the closure touches Python state, which is not
    // `UnwindSafe`, but nothing observes that state after a panic -- the only
    // thing built here is an error value.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(result) => result,
        Err(payload) => Err(ParseError::new_err(format!(
            "internal parser panic: {} (this is a bug in fast_mail_parser; \
             please report it with the input that triggered it)",
            panic_message(payload.as_ref())
        ))),
    }
}
