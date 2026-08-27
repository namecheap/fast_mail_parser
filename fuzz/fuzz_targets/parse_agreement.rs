//! Fuzz target for the newer APIs, checking them against the flat parse.
//!
//! `parse_email` has had a fuzz target since #156. The two APIs added since --
//! `parse_email_metadata` (#97) and `parse_email_tree` (#99) -- are re-derivations
//! of the same message, and the interesting bugs in a re-derivation are the ones
//! where it disagrees with the original. So this target asserts agreement rather
//! than only absence of panics.
//!
//! Invariants on arbitrary input:
//!
//! 1. **No panic** in either new API, same as the flat target.
//! 2. **Metadata mode succeeds wherever the full parse does.** It does strictly
//!    less work: it never transfer-decodes, and `get_body_encoded` cannot fail.
//!    So the only way it can fail is a failure the full parse shares. The
//!    converse is deliberately NOT asserted -- a broken transfer encoding fails
//!    the full parse and passes metadata, which is a documented difference.
//! 3. **The envelope agrees.** Subject, date and the whole header map must be
//!    identical between the two modes. A triage sweep and a full parse
//!    disagreeing about the same message is the failure this exists to prevent.
//! 4. **The attachment inventory agrees** -- count, and per attachment the
//!    mimetype, filename, content id and disposition. Only the content and the
//!    size differ by design.
//! 5. **Decoding does not amplify a part beyond doubling it.** Not "decoded is
//!    never larger than encoded", which is false: quoted-printable emits a line
//!    break as CRLF, so a body of bare LFs grows by a byte per line.
//! 6. **The tree is bounded and deterministic**, every attachment the flat parse
//!    reports appears as some leaf's content, and the tree root's headers are the
//!    flat parse's headers -- the root is the same message, so they cannot differ
//!    without one derivation having drifted from the other.

#![no_main]

// The core is included by path rather than linked: see fuzz/Cargo.toml.
#[path = "../../src/mail_parser.rs"]
mod mail_parser;

use libfuzzer_sys::fuzz_target;

/// Render a tree deterministically, for the parse-twice comparison.
fn canonical_tree(part: &mail_parser::MimePart) -> String {
    let mut out = String::new();
    render(part, 0, &mut out);
    out
}

fn render(part: &mail_parser::MimePart, depth: usize, out: &mut String) {
    out.push_str(&format!(
        "{depth}|{}|{}|{:?}|{:?}|{}|{:?}\n",
        part.content_type,
        part.filename,
        part.content_id,
        part.disposition,
        part.is_message,
        part.content.as_ref().map(|bytes| bytes.len()),
    ));
    for (name, values) in &part.headers {
        out.push_str(&format!("{depth}h|{name}|{values:?}\n"));
    }
    for child in &part.children {
        render(child, depth + 1, out);
    }
}

/// Every leaf's content, for the containment check against the flat parse.
fn leaf_contents(part: &mail_parser::MimePart, out: &mut Vec<Vec<u8>>) {
    if let Some(content) = &part.content {
        out.push(content.clone());
    }
    for child in &part.children {
        leaf_contents(child, out);
    }
}

fuzz_target!(|data: &[u8]| {
    let full = mail_parser::parse_email(data);
    let metadata = mail_parser::parse_email_metadata(data);

    if let Ok(full) = &full {
        let metadata = metadata.expect(
            "metadata mode failed where the full parse succeeded, though it does \
             strictly less work",
        );

        assert_eq!(full.subject, metadata.subject, "subject disagrees");
        assert_eq!(full.date, metadata.date, "date disagrees");
        assert_eq!(full.headers, metadata.headers, "header map disagrees");

        assert_eq!(
            full.attachments.len(),
            metadata.attachments.len(),
            "attachment count disagrees"
        );

        for (decoded, described) in full.attachments.iter().zip(&metadata.attachments) {
            assert_eq!(decoded.mimetype, described.mimetype, "mimetype disagrees");
            assert_eq!(decoded.filename, described.filename, "filename disagrees");
            assert_eq!(
                decoded.content_id, described.content_id,
                "content id disagrees"
            );
            assert_eq!(
                decoded.disposition, described.disposition,
                "disposition disagrees"
            );

            // Decoding cannot amplify a part beyond doubling it.
            //
            // The obvious invariant -- decoded is never larger than encoded --
            // is false, and this target found it in fifteen minutes with 45
            // crashers, every one quoted-printable. The `quoted_printable`
            // crate emits a line break as `\r\n` (`decoded.push(b'\r');
            // decoded.push(b'\n')`), so in robust mode a body of bare LFs comes
            // back one byte longer per line: the minimised case was a body of a
            // single `\n` decoding to two bytes.
            //
            // Doubling is the true bound. Every `\r\n` pair costs at least one
            // input byte, `=XX` shrinks three to one, a literal byte is one to
            // one, and base64 shrinks by a quarter. Past 2x, something is
            // amplifying.
            assert!(
                decoded.content.len() <= described.encoded_size * 2,
                "decoded size {} is more than double the encoded size {}",
                decoded.content.len(),
                described.encoded_size
            );
        }
    }

    let first = mail_parser::parse_email_tree(data);
    let second = mail_parser::parse_email_tree(data);

    match (&first, &second) {
        (Ok(a), Ok(b)) => {
            assert_eq!(
                canonical_tree(a),
                canonical_tree(b),
                "parsing the same input into a tree twice produced different results"
            );

            // Whatever the flat parse calls an attachment must be somewhere in
            // the tree: the tree keeps parts the projection drops, never fewer.
            if let Ok(full) = &full {
                // The root of the tree is the same message the flat parse read,
                // so its headers are the flat parse's headers. This is the gap
                // that let metadata mode drift: a repair reached one derivation
                // and not another, and nothing compared them. Cheap to assert,
                // and it fails the moment the two paths stop agreeing about what
                // a message's headers are.
                assert_eq!(
                    full.headers, a.headers,
                    "the tree root's headers disagree with the flat parse"
                );

                let mut leaves = Vec::new();
                leaf_contents(a, &mut leaves);
                for attachment in &full.attachments {
                    assert!(
                        leaves.contains(&attachment.content),
                        "an attachment's bytes are missing from the tree"
                    );
                }
            }
        }
        (Err(_), Err(_)) => {}
        _ => panic!("parsing the same input into a tree twice disagreed on success"),
    }
});
