//! Fuzz target for the newer APIs, checking them against the flat parse.
//!
//! `parse_email` has had a fuzz target since #156. The APIs added since --
//! `parse_email_metadata` (#97), `parse_email_tree` (#99) and
//! `parse_email_lazy` (#97) -- are re-derivations of the same message, and the
//! interesting bugs in a re-derivation are the ones where it disagrees with the
//! original. So this target asserts agreement rather than only absence of panics.
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
//! 7. **A deferred decode equals the full parse's content.** This is the load-
//!    bearing one for lazy mode: it retains a copy of each part exactly as it sits
//!    in the message and decodes that copy on demand, which reproduces the full
//!    parse's bytes only if mailparse's `raw_bytes` really is that part and
//!    nothing else. Arbitrary input is the right place to test a claim about a
//!    parser's slicing. The envelope, the bodies and the whole warning list must
//!    agree too -- the last of those is what lets `strict=True` mean the same
//!    thing in both modes.
//! 8. **A tree is the same tree in all three modes** (#202). `mode=` on
//!    `parse_email_tree` adds two more derivations of one message, so it adds the
//!    failure this target exists for. Every per-node field except the body must be
//!    identical to full mode's, and a deferred leaf must decode to exactly the
//!    bytes full mode decoded -- which extends the `raw_bytes` claim above to a
//!    case flat lazy mode never reaches, a message whose *root* is the leaf.

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

/// A warning list as comparable values. `Warning` is deliberately not `PartialEq`
/// -- it is a report, not a key -- so the comparison spells out what "the same
/// warnings" means: same kinds, same locators, same wording, same order.
fn rendered_warnings(warnings: &[mail_parser::Warning]) -> Vec<(&str, &str, &str)> {
    warnings
        .iter()
        .map(|warning| {
            (
                warning.kind,
                warning.part_path.as_str(),
                warning.detail.as_str(),
            )
        })
        .collect()
}

/// A deferred tree rendered like `canonical_tree`, minus the body, so the three
/// modes can be compared on everything except what they deliberately differ on.
fn canonical_node(node: &mail_parser::TreeNode) -> String {
    let mut out = String::new();
    render_node(node, 0, &mut out);
    out
}

fn render_node(node: &mail_parser::TreeNode, depth: usize, out: &mut String) {
    out.push_str(&format!(
        "{depth}|{}|{}|{:?}|{:?}|{}\n",
        node.content_type, node.filename, node.content_id, node.disposition, node.is_message,
    ));
    for (name, values) in &node.headers {
        out.push_str(&format!("{depth}h|{name}|{values:?}\n"));
    }
    for child in &node.children {
        render_node(child, depth + 1, out);
    }
}

/// The same rendering for a full-mode tree, so the two can be compared directly.
fn canonical_full(part: &mail_parser::MimePart) -> String {
    let mut out = String::new();
    render_full(part, 0, &mut out);
    out
}

fn render_full(part: &mail_parser::MimePart, depth: usize, out: &mut String) {
    out.push_str(&format!(
        "{depth}|{}|{}|{:?}|{:?}|{}\n",
        part.content_type, part.filename, part.content_id, part.disposition, part.is_message,
    ));
    for (name, values) in &part.headers {
        out.push_str(&format!("{depth}h|{name}|{values:?}\n"));
    }
    for child in &part.children {
        render_full(child, depth + 1, out);
    }
}

/// Every node's body as the full parse produced it, in walk order.
fn full_bodies(part: &mail_parser::MimePart, out: &mut Vec<Option<Vec<u8>>>) {
    out.push(part.content.clone());
    for child in &part.children {
        full_bodies(child, out);
    }
}

/// Every node's body as a deferred tree would produce it: decoded from what the
/// node retained, or `None` for a container.
///
/// A leaf that cannot decode from its retained bytes returns an `Err`, which is a
/// failure of the retention rather than of the message -- the full parse decoded
/// the same part.
fn deferred_bodies(
    node: &mail_parser::TreeNode,
    out: &mut Vec<Option<Vec<u8>>>,
) -> Result<(), ()> {
    match &node.body {
        mail_parser::NodeBody::Container => out.push(None),
        mail_parser::NodeBody::Decoded { content, .. } => out.push(Some(content.clone())),
        mail_parser::NodeBody::Undecoded { raw, .. } => {
            let raw = raw.as_ref().ok_or(())?;
            out.push(Some(mail_parser::decode_part(raw).map_err(|_| ())?));
        }
    }
    for child in &node.children {
        deferred_bodies(child, out)?;
    }
    Ok(())
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

/// Panic on purpose when `FMP_FUZZ_CANARY` is set, so this target's own path to
/// a filed issue can be rehearsed.
///
/// Duplicated from `parse_email.rs` on purpose. The canary has to live in every
/// target, because the deep run is a matrix and its drill verdict is decided per
/// job: a target without one reports "the canary did not crash, so the alarm is
/// not working", which is true of that job and misleading about the alarm. That
/// is exactly what the first drill after the matrix landed did.
///
/// Armed through the environment rather than an input: libFuzzer learns the
/// operands of comparisons against input and would find a magic value, and a
/// planted input persists in the cached corpus. Both of those happened.
fn canary_armed() -> bool {
    std::env::var_os("FMP_FUZZ_CANARY").is_some_and(|value| !value.is_empty())
}

fuzz_target!(|data: &[u8]| {
    if canary_armed() {
        panic!("fuzz canary: this crash is a deliberate test of the reporting path");
    }

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

    let lazy = mail_parser::parse_email_lazy(data);

    if let Ok(full) = &full {
        let lazy = lazy.expect(
            "lazy mode failed where the full parse succeeded, though it decodes \
             strictly less: it defers the attachments and reads the same bodies",
        );

        assert_eq!(full.subject, lazy.subject, "subject disagrees");
        assert_eq!(full.date, lazy.date, "date disagrees");
        assert_eq!(full.headers, lazy.headers, "header map disagrees");
        assert_eq!(full.text_plain, lazy.text_plain, "text_plain disagrees");
        assert_eq!(full.text_html, lazy.text_html, "text_html disagrees");
        assert_eq!(
            rendered_warnings(&full.warnings),
            rendered_warnings(&lazy.warnings),
            "the warning lists disagree, so strict mode would not agree either"
        );

        assert_eq!(
            full.attachments.len(),
            lazy.attachments.len(),
            "attachment count disagrees"
        );

        for (decoded, deferred) in full.attachments.iter().zip(&lazy.attachments) {
            assert_eq!(decoded.mimetype, deferred.mimetype, "mimetype disagrees");
            assert_eq!(decoded.filename, deferred.filename, "filename disagrees");
            assert_eq!(
                decoded.content_id, deferred.content_id,
                "content id disagrees"
            );
            assert_eq!(
                decoded.disposition, deferred.disposition,
                "disposition disagrees"
            );

            // The claim lazy mode rests on: the retained part decodes to exactly
            // what the full parse decoded from it. A part the full parse decoded
            // must also decode from its retained bytes -- if it cannot, the
            // retained slice is not the part.
            let content = mail_parser::decode_part(&deferred.raw).expect(
                "a part the full parse decoded failed to decode from its \
                 retained bytes",
            );
            assert_eq!(
                decoded.content, content,
                "the deferred decode disagrees with the full parse"
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

    // The deferred tree modes (#202): two more derivations of the same message,
    // so two more chances for one to drift from the others.
    let described = mail_parser::parse_tree_deferred(data, false);
    let deferred = mail_parser::parse_tree_deferred(data, true);

    if let Ok(full) = &first {
        let described = described.expect(
            "metadata mode failed to build a tree the full mode built, though it \
             decodes strictly less: only a message/rfc822 body, which full mode \
             decodes too",
        );
        let deferred = deferred.expect(
            "lazy mode failed to build a tree the full mode built, though it \
             decodes strictly less",
        );

        let shape = canonical_full(full);
        assert_eq!(
            shape,
            canonical_node(&described),
            "the metadata tree's shape disagrees with full mode"
        );
        assert_eq!(
            shape,
            canonical_node(&deferred),
            "the lazy tree's shape disagrees with full mode"
        );

        // Which nodes have a body of their own, and how big it is on the wire,
        // must be one answer across the two deferred modes -- `encoded_size is
        // None` is the same question as `content is None`.
        let mut sizes = Vec::new();
        node_sizes(&described, &mut sizes);
        let mut deferred_sizes = Vec::new();
        node_sizes(&deferred, &mut deferred_sizes);
        assert_eq!(
            sizes, deferred_sizes,
            "the two deferred modes disagree about encoded sizes"
        );

        let mut expected = Vec::new();
        full_bodies(full, &mut expected);
        assert_eq!(
            sizes.len(),
            expected.len(),
            "the deferred tree has a different number of nodes"
        );
        for (size, body) in sizes.iter().zip(&expected) {
            assert_eq!(
                size.is_none(),
                body.is_none(),
                "a node reports a body in one mode and not the other"
            );
            if let (Some(size), Some(body)) = (size, body) {
                // The same bound the flat modes assert: decoding cannot amplify
                // a part beyond doubling it.
                assert!(
                    body.len() <= size * 2,
                    "decoded size {} is more than double the encoded size {size}",
                    body.len()
                );
            }
        }

        // The claim the lazy tree rests on, and the one flat lazy mode cannot
        // reach: a retained *root* re-parses to the part it was taken from.
        let mut produced = Vec::new();
        deferred_bodies(&deferred, &mut produced)
            .expect("a node the full parse decoded failed to decode from its retained bytes");
        assert_eq!(
            expected, produced,
            "a deferred decode disagrees with the full parse"
        );
    }
});

/// Every node's `encoded_size`, in walk order.
fn node_sizes(node: &mail_parser::TreeNode, out: &mut Vec<Option<usize>>) {
    out.push(node.body.encoded_size());
    for child in &node.children {
        node_sizes(child, out);
    }
}
