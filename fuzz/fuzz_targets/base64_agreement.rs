//! Fuzz target for the SIMD base64 fast path (#228).
//!
//! `decode_base64` tries `base64_simd::STANDARD` first and falls back to
//! `data_encoding::BASE64_MIME_PERMISSIVE` on rejection. That is only sound if
//! the set "SIMD accepts, data-encoding rejects" is empty, and if everything both
//! accept decodes to the same bytes -- otherwise the fast path silently changes
//! what this library accepts, which is a correctness change wearing a performance
//! change's clothes.
//!
//! The vendored `simd_and_data_encoding_agree` test asserts that over a
//! hand-built corpus. This asserts it over arbitrary input, through the real
//! entry point rather than the private function:
//!
//! 1. **Acceptance agrees.** If `get_body_raw` succeeds, the oracle succeeds, and
//!    vice versa.
//! 2. **Bytes agree** on everything accepted.
//! 3. **The error agrees** on everything rejected -- not merely that both failed,
//!    but that the message is the one `data_encoding` produced, since that string
//!    reaches Python as `DecodeError`'s text.
//!
//! The oracle is exactly what this function did before the fast path existed.

#![no_main]

use libfuzzer_sys::fuzz_target;

/// Deliberate crash, to prove the reporting path works end to end.
///
/// The value must be non-empty, not merely present: a workflow `env:` entry whose
/// expression evaluates to `''` still defines the variable.
fn canary_armed() -> bool {
    std::env::var_os("FMP_FUZZ_CANARY").is_some_and(|value| !value.is_empty())
}

/// What `decode_base64` did before #228.
fn oracle(body: &[u8]) -> Result<Vec<u8>, data_encoding::DecodeError> {
    let cleaned: Vec<u8> = body
        .iter()
        .filter(|c| !c.is_ascii_whitespace())
        .cloned()
        .collect();
    data_encoding::BASE64_MIME_PERMISSIVE.decode(&cleaned)
}

fuzz_target!(|data: &[u8]| {
    if canary_armed() {
        panic!("fuzz canary: this crash is a deliberate test of the reporting path");
    }

    // A header block is not interesting here and a malformed one would only
    // shrink the space of bodies reached, so it is fixed and the whole input is
    // the body.
    let mut message = Vec::with_capacity(data.len() + 64);
    message.extend_from_slice(b"Content-Transfer-Encoding: base64\r\n\r\n");
    message.extend_from_slice(data);

    let Ok(parsed) = mailparse::parse_mail(&message) else {
        // Header parsing is another target's problem.
        return;
    };

    match (parsed.get_body_raw(), oracle(data)) {
        (Ok(got), Ok(want)) => assert_eq!(
            got, want,
            "SIMD and data-encoding decoded the same body to different bytes"
        ),
        (Err(got), Err(want)) => assert_eq!(
            got.to_string(),
            format!("Base64 decode error: {want}"),
            "the two decoders rejected the same body with different errors"
        ),
        (Ok(got), Err(want)) => panic!(
            "the fast path accepted {} bytes that data-encoding rejects ({want}) \
             -- this is the unsound case the fallback design rules out",
            got.len()
        ),
        (Err(got), Ok(want)) => panic!(
            "rejected a body data-encoding decodes to {} bytes: {got}",
            want.len()
        ),
    }
});
