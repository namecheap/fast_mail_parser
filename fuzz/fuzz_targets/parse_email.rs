//! Fuzz target for the parsing core.
//!
//! Invariants asserted on arbitrary input:
//!
//! 1. **No panic.** Still the invariant that matters most, though not for the
//!    reason first written here: a panic never crashed the host process -- PyO3
//!    catches it -- and since #162 the binding converts it to a `ParseError`.
//!    What a panic costs is a message parsed as garbage instead of parsed, so
//!    finding one here is finding it before a mail pipeline does. libFuzzer
//!    treats any panic as a finding.
//! 2. **Determinism.** The same bytes parse to the same result twice, compared
//!    over every field via `canonical`, headers **in order**. This target's first
//!    run failed here, on a header map whose iteration order was randomised per
//!    instance -- a real bug (#157), not a flawed check. Now that the core keeps
//!    wire order, an ordering regression is a finding rather than noise.
//! 3. **Bounded output.** Decoded output stays within a generous multiple of the
//!    input, so no input can make the parser amplify its way through memory.

#![no_main]

// The core is included by path rather than linked: see fuzz/Cargo.toml.
#[path = "../../src/mail_parser.rs"]
mod mail_parser;

use libfuzzer_sys::fuzz_target;

/// Render a parse deterministically for comparison.
///
/// Headers are compared **in order**, deliberately. This target's first run
/// failed here because `headers` was a `HashMap` whose iteration order is
/// randomised per instance -- which turned out to be a real bug rather than only
/// a flawed check (#157), since that order reached callers. Now that the core
/// keeps wire order, comparing unsorted means this target also guards against a
/// regression to nondeterministic ordering.
fn canonical(mail: &mail_parser::Mail) -> String {
    let headers = &mail.headers;

    let attachments: Vec<_> = mail
        .attachments
        .iter()
        .map(|a| (&a.mimetype, &a.filename, &a.content, &a.content_id, &a.disposition))
        .collect();

    format!(
        "{:?}",
        (
            &mail.subject,
            &mail.text_plain,
            &mail.text_html,
            &mail.date,
            &mail.from_,
            &mail.to,
            &mail.cc,
            &mail.bcc,
            &mail.reply_to,
            attachments,
            headers,
        )
    )
}

/// How much larger than its input a parse is allowed to get.
///
/// Transfer decoding only shrinks, but charset decoding can grow: latin-1 to
/// UTF-8 at most doubles, and headers are additionally copied into the header
/// map. Eight times is far above anything legitimate while still catching real
/// amplification, and is deliberately loose -- a tight bound here would report
/// false findings rather than bugs.
const MAX_OUTPUT_FACTOR: usize = 8;

fn total_output_bytes(mail: &mail_parser::Mail) -> usize {
    let bodies: usize = mail
        .text_plain
        .iter()
        .chain(mail.text_html.iter())
        .map(String::len)
        .sum();
    let attachments: usize = mail
        .attachments
        .iter()
        .map(|attachment| attachment.content.len() + attachment.filename.len())
        .sum();
    let headers: usize = mail
        .headers
        .iter()
        .map(|(key, values)| key.len() + values.iter().map(String::len).sum::<usize>())
        .sum();
    bodies + attachments + headers + mail.subject.len() + mail.date.len()
}

/// Input that makes this target panic on purpose.
///
/// The scheduled deep run is supposed to turn a crasher into an artifact and a
/// filed issue, and #102 asks for that path to be verified rather than trusted.
/// Verifying it needs a crash on demand, and the whole point of the harness is
/// that no known input produces one -- so this provides it.
///
/// Dispatching the deep-fuzz workflow with `inject_canary` writes these bytes
/// into the corpus, and libFuzzer runs corpus inputs first. Nothing else reaches
/// it: a 32-byte magic string is far outside what random mutation finds, and
/// nothing in the seed corpus resembles it.
const CANARY: &[u8] = b"FMP_FUZZ_CANARY_DO_NOT_REPORT_42";

fuzz_target!(|data: &[u8]| {
    if data == CANARY {
        panic!("fuzz canary: this crash is a deliberate test of the reporting path");
    }

    let first = mail_parser::parse_email(data);
    let second = mail_parser::parse_email(data);

    match (&first, &second) {
        (Ok(a), Ok(b)) => {
            assert_eq!(
                canonical(a),
                canonical(b),
                "parsing the same input twice produced different results"
            );

            let produced = total_output_bytes(a);
            let allowed = data.len().saturating_mul(MAX_OUTPUT_FACTOR) + 4096;
            assert!(
                produced <= allowed,
                "output amplification: {produced} bytes from {} of input",
                data.len()
            );
        }
        (Err(_), Err(_)) => {}
        _ => panic!("parsing the same input twice disagreed on success"),
    }
});
