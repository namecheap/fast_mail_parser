"""The structural API: `parse_email_tree` and `walk` (#99).

`parse_email` returns a flattened projection -- bodies in one list, attachments in
another, containers dropped -- and every flattening loses something. These tests
pin what the tree keeps that the projection cannot: sibling relationships inside
`multipart/alternative`, and the inside of an embedded `message/rfc822`.

The strongest oracle available is the stdlib. `email.message.Message.walk` yields
the same topology this tree should have, so the parity test below compares the two
over the whole corpus rather than over chosen examples.
"""
import email
import email.policy
import glob
import inspect
import os

import pytest

from fast_mail_parser import (
    DecodeError,
    HeaderParseError,
    MimeStructureError,
    parse_email,
    parse_email_tree,
    walk,
)

_DATA = os.path.join(os.path.dirname(__file__), "data")

# The same corpus as test_stdlib_parity.py, and like it, nothing is excluded.
# invalid_message.eml was, because the two parsers disagreed about its structure
# while only the stdlib recovered from its unterminated header block; the tree
# path applies the same repair as the flat one, so the topologies now agree (#150).
FIXTURES = sorted(glob.glob(os.path.join(_DATA, "rfc", "*.eml"))) + sorted(
    glob.glob(os.path.join(_DATA, "*.eml"))
)


def _ids(paths):
    return [os.path.basename(path) for path in paths]


# --- topology against the stdlib ----------------------------------------------


@pytest.mark.parametrize("path", FIXTURES, ids=_ids(FIXTURES))
def test__tree_topology_matches_the_stdlib(path: str):
    with open(path, "rb") as handle:
        raw = handle.read()

    ours = [part.content_type for part in walk(parse_email_tree(raw))]
    theirs = [
        part.get_content_type()
        for part in email.message_from_bytes(raw, policy=email.policy.default).walk()
    ]

    assert ours == theirs


# --- what the flat projection cannot say ---------------------------------------


ALTERNATIVE = (
    b"From: sender@example.com\r\n"
    b"Subject: alternative\r\n"
    b'Content-Type: multipart/alternative; boundary="bnd"\r\n'
    b"\r\n"
    b"--bnd\r\n"
    b"Content-Type: text/plain\r\n"
    b"\r\n"
    b"plain version\r\n"
    b"--bnd\r\n"
    b"Content-Type: text/html\r\n"
    b"\r\n"
    b"<p>html version</p>\r\n"
    b"--bnd--\r\n"
)


def test__alternative_siblings_share_a_parent():
    # The relationship `parse_email` loses: it reports one text_plain and one
    # text_html with nothing to say they are two renderings of one thing.
    root = parse_email_tree(ALTERNATIVE)

    assert root.content_type == "multipart/alternative"
    assert [child.content_type for child in root.children] == [
        "text/plain",
        "text/html",
    ]
    assert root.content is None, "a container's body is its children"


def test__a_container_reports_no_content_but_its_leaves_do():
    root = parse_email_tree(ALTERNATIVE)

    assert root.content is None
    assert [child.content for child in root.children] == [
        b"plain version",
        b"<p>html version</p>",
    ]


# --- embedded messages ---------------------------------------------------------


def _bounce(inner: bytes) -> bytes:
    return (
        b"From: postmaster@example.com\r\n"
        b"Subject: Delivery failure\r\n"
        b'Content-Type: multipart/mixed; boundary="outer"\r\n'
        b"\r\n"
        b"--outer\r\n"
        b"Content-Type: text/plain\r\n"
        b"\r\n"
        b"Your message could not be delivered.\r\n"
        b"--outer\r\n"
        b"Content-Type: message/rfc822\r\n"
        b"\r\n" + inner + b"\r\n--outer--\r\n"
    )


ORIGINAL = (
    b"From: alice@example.com\r\n"
    b"To: nobody@example.invalid\r\n"
    b"Subject: the original message\r\n"
    b"Content-Type: text/plain\r\n"
    b"\r\n"
    b"hello\r\n"
)


def test__an_embedded_message_is_parsed_not_opaque():
    root = parse_email_tree(_bounce(ORIGINAL))

    embedded = [part for part in walk(root) if part.is_message]
    assert len(embedded) == 1

    inner_root = embedded[0].children[0]
    # The point: the bounced message's own headers are reachable. Through
    # `parse_email` this is one attachment blob to re-parse by hand.
    assert inner_root.headers["Subject"] == ["the original message"]
    assert inner_root.headers["From"] == ["alice@example.com"]
    # Trailing CRLF included: it is part of the body, not of the boundary
    # delimiter, since the embedded message ends before the outer boundary line.
    assert inner_root.content == b"hello\r\n"


def test__a_doubly_nested_message_keeps_both_levels():
    root = parse_email_tree(_bounce(_bounce(ORIGINAL)))

    depths = [part.content_type for part in walk(root)]

    # Two embedded messages, one inside the other.
    assert depths.count("message/rfc822") == 2
    subjects = [
        part.headers["Subject"][0] for part in walk(root) if "Subject" in part.headers
    ]
    assert "the original message" in subjects


def test__embedded_message_nesting_counts_against_the_recursion_cap():
    # An onion of forwards must not be able to recurse further than a multipart
    # tree can. 300 exceeds MAX_MIME_DEPTH (256).
    payload = ORIGINAL
    for _ in range(300):
        payload = (
            b"Content-Type: message/rfc822\r\n\r\n" + payload
        )

    with pytest.raises(MimeStructureError):
        parse_email_tree(payload)


# --- consistency with the flat API ---------------------------------------------


@pytest.mark.parametrize("path", FIXTURES, ids=_ids(FIXTURES))
def test__leaf_content_agrees_with_the_flat_api(path: str):
    with open(path, "rb") as handle:
        raw = handle.read()

    flat = parse_email(raw)
    tree_leaves = [
        part.content
        for part in walk(parse_email_tree(raw))
        if part.content is not None
    ]

    # Every attachment's bytes appear as some leaf's content. The tree keeps
    # parts the projection drops, so this is containment rather than equality.
    for attachment in flat.attachments:
        assert attachment.content in tree_leaves, (
            f"{os.path.basename(path)}: attachment bytes missing from the tree"
        )


def test__str_and_bytes_payloads_give_the_same_tree():
    from_bytes = parse_email_tree(ALTERNATIVE)
    from_str = parse_email_tree(ALTERNATIVE.decode("ascii"))

    assert [p.content_type for p in walk(from_str)] == [
        p.content_type for p in walk(from_bytes)
    ]


# --- walk ----------------------------------------------------------------------


def test__walk_is_depth_first_and_yields_the_root_first():
    root = parse_email_tree(_bounce(ORIGINAL))

    order = [part.content_type for part in walk(root)]

    assert order[0] == "multipart/mixed"
    assert order == [
        "multipart/mixed",
        "text/plain",
        "message/rfc822",
        "text/plain",
    ]


def test__walk_is_a_generator_not_a_materialised_list():
    # So that stopping at the first match of something costs nothing for the
    # rest of the tree -- which is the usual reason to walk one.
    walker = walk(parse_email_tree(_bounce(ORIGINAL)))

    assert inspect.isgenerator(walker)
    assert next(walker).content_type == "multipart/mixed"


# --- errors --------------------------------------------------------------------


# Borrowed from test_error_taxonomy.py, where these are proven to raise. Random
# bytes are a bad choice here: a payload with no colon anywhere still parses, as
# #150 established the hard way.
MALFORMED_HEADERS = b" unexpected continuation\r\n\r\nbody"
BROKEN_BASE64_BODY = (
    b"Subject: broken\r\n"
    b"Content-Type: text/plain; charset=utf-8\r\n"
    b"Content-Transfer-Encoding: base64\r\n"
    b"\r\n"
    b"!!!! not base64 !!!!\r\n"
)


def test__the_tree_api_raises_the_same_errors():
    with pytest.raises(HeaderParseError):
        parse_email_tree(MALFORMED_HEADERS)

    # A leaf whose transfer encoding is broken fails the tree parse too, rather
    # than yielding a part with silently empty content.
    with pytest.raises(DecodeError):
        parse_email_tree(BROKEN_BASE64_BODY)


def test__a_non_payload_is_rejected():
    with pytest.raises(TypeError):
        parse_email_tree(42)
