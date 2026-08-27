//! Fuzz target for the parsing core.
//!
//! Invariants asserted on arbitrary input:
//!
//! 1. **No panic.** A panic reaching the FFI boundary is a crash in the host
//!    process, so for a library that parses attacker-controlled mail this is the
//!    invariant that matters most. libFuzzer treats any panic as a finding.
//! 2. **Determinism.** The same bytes parse to the same result twice. Anything
//!    else means hidden state, and would make every other test unreliable.
//! 3. **Bounded output.** Decoded output stays within a generous multiple of the
//!    input, so no input can make the parser amplify its way through memory.

#![no_main]

// The core is included by path rather than linked: see fuzz/Cargo.toml.
#[path = "../../src/mail_parser.rs"]
mod mail_parser;

use libfuzzer_sys::fuzz_target;

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

fuzz_target!(|data: &[u8]| {
    let first = mail_parser::parse_email(data);
    let second = mail_parser::parse_email(data);

    match (&first, &second) {
        (Ok(a), Ok(b)) => {
            // `Mail` has no PartialEq, and Debug covers every field, so this
            // catches a difference anywhere in the result rather than in a
            // hand-picked subset.
            assert_eq!(
                format!("{a:?}"),
                format!("{b:?}"),
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
