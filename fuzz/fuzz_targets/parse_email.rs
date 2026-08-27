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
            // Included so "every field" stays true, and because the order of
            // repairs is part of the contract: strict mode reports the first
            // one, so a nondeterministic order would make it nondeterministic
            // too. Compared unsorted, deliberately.
            &mail.warnings,
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
    // Warning text counts too. A repair emits a constant-ish detail string plus
    // a copy of the label or value it could not read, and triggering one costs
    // fewer input bytes than it emits -- roughly 3.5x for a message that is
    // nothing but parts with unrecognised charsets. That is comfortably inside
    // the factor below, and counting it is what would catch a future kind whose
    // detail is long enough not to be.
    let warnings: usize = mail
        .warnings
        .iter()
        .map(|w| w.kind.len() + w.part_path.len() + w.detail.len())
        .sum();
    bodies + attachments + headers + warnings + mail.subject.len() + mail.date.len()
}

/// Panic on purpose when `FMP_FUZZ_CANARY` is set in the environment.
///
/// The scheduled deep run turns a crasher into an artifact and a filed issue, and
/// #102 asks for that path to be verified rather than trusted. Verifying it needs
/// a crash on demand, and the point of the harness is that no known input
/// produces one -- so the drill has to stage one.
///
/// It is staged through the **environment**, not through input bytes, and the
/// first version got this wrong twice over. A magic 32-byte input looked
/// unreachable by chance, and libFuzzer found it within seconds: it intercepts
/// `memcmp` and learns the operands it is compared against, so a literal
/// comparison against input is an instruction to the fuzzer, not a secret from
/// it. (`PersAutoDict` in its own log is that machinery at work.) The bytes also
/// had to be planted in the corpus, and the corpus is cached, so each drill left
/// its staged crash behind for later runs to trip over.
///
/// An environment variable has neither problem. No input can synthesise it, and
/// nothing is written where it can persist.
///
/// The value must be non-empty, not merely present. A workflow `env:` entry whose
/// expression evaluates to `''` still *defines* the variable, so `is_some()` was
/// true on every run and armed the canary unconditionally -- a third false
/// crasher, from the fix for the second.
fn canary_armed() -> bool {
    std::env::var_os("FMP_FUZZ_CANARY").is_some_and(|value| !value.is_empty())
}

fuzz_target!(|data: &[u8]| {
    if canary_armed() {
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
