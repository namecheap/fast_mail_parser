//! Fuzz target for the run-copying quoted-printable decoder (#229).
//!
//! `vendor/mailparse/src/qp.rs` replaces `quoted_printable::decode(.., Robust)`
//! for message bodies. It is only a performance change if its output is
//! byte-identical on every input, so that is what this asserts, through the
//! public entry point rather than the private function:
//!
//! 1. **No panic**, on arbitrary bytes in a quoted-printable body.
//! 2. **Bytes agree** with the crate this replaced, always -- Robust mode has no
//!    rejection path, so there is no error case to compare, only output.
//!
//! The oracle is pinned to `=0.5.1`, the version the extension links. 0.5.2
//! changed what Robust mode emits for a body whose last line ends in a soft
//! break, and an unpinned requirement would silently make this target assert the
//! wrong thing. See `vendor/mailparse/PATCH.md`.

#![no_main]

use libfuzzer_sys::fuzz_target;

/// Deliberate crash, to prove the reporting path works end to end.
///
/// The value must be non-empty, not merely present: a workflow `env:` entry whose
/// expression evaluates to `''` still defines the variable.
fn canary_armed() -> bool {
    std::env::var_os("FMP_FUZZ_CANARY").is_some_and(|value| !value.is_empty())
}

fuzz_target!(|data: &[u8]| {
    if canary_armed() {
        panic!("fuzz canary: this crash is a deliberate test of the reporting path");
    }

    let mut message = Vec::with_capacity(data.len() + 64);
    message.extend_from_slice(b"Content-Transfer-Encoding: quoted-printable\r\n\r\n");
    message.extend_from_slice(data);

    let Ok(parsed) = mailparse::parse_mail(&message) else {
        // Header parsing is another target's problem.
        return;
    };

    let got = parsed
        .get_body_raw()
        .expect("Robust quoted-printable decoding never fails");
    let want = quoted_printable::decode(data, quoted_printable::ParseMode::Robust)
        .expect("Robust quoted-printable decoding never fails");

    assert_eq!(
        got, want,
        "the run-copying decoder and quoted_printable 0.5.1 disagree"
    );
});
