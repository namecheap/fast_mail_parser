"""`parse_email_tree(payload, mode=...)` (#202).

The tree is where the modes are worth the most and where nothing had them: a
`mode="full"` tree decodes every leaf, so walking the structure of a large
message to pull one part out of it decoded all the others first.

`mode="lazy"` returns `PyLazyMimePart` nodes whose `content` decodes on first
access and caches. `mode="metadata"` returns `PyMimePartMetadata` nodes, which
decode nothing and retain nothing and report `encoded_size` in place of
`content`.

Three things have to hold, and this file is organised as them.

**The tree is the same tree in every mode.** Not "similar": the topology and every
per-node field except the body must be equal across all three, over the whole
corpus. A caller who switches mode to save work must not be switching what the
message looks like. The `parse_agreement` fuzz target asserts the same on
arbitrary input.

**A leaf's bytes must equal full mode's.** Lazy mode retains each part exactly as
it sits in the message and re-parses that copy on access, which reproduces full
mode's `content` only if mailparse's `raw_bytes` really is that part -- including
for a *root*, which flat lazy mode never exercises because it defers only
subparts.

**A leaf nobody reads must never be decoded.** `is_decoded` makes that an
assertion rather than a timing argument.
"""
import glob
import os

import pytest

from fast_mail_parser import (
    DecodeError,
    HeaderParseError,
    MimeStructureError,
    PyLazyMimePart,
    PyMimePart,
    PyMimePartMetadata,
    parse_email,
    parse_email_tree,
    walk,
)

_DATA = os.path.join(os.path.dirname(__file__), "data")

# The same corpus as test_mime_tree.py, and like it nothing is excluded --
# invalid_message.eml included, since every mode applies the #150 repair and so
# the three must agree about it too.
FIXTURES = sorted(glob.glob(os.path.join(_DATA, "rfc", "*.eml"))) + sorted(
    glob.glob(os.path.join(_DATA, "*.eml"))
)
IDS = [os.path.basename(path) for path in FIXTURES]

MODES = ["lazy", "metadata"]

EXPECTED_LAZY_NODE_ATTRS = {
    "content_type",
    "headers",
    "filename",
    "content_id",
    "disposition",
    "is_message",
    "encoded_size",
    "content",
    "is_decoded",
    "children",
}
EXPECTED_METADATA_NODE_ATTRS = {
    "content_type",
    "headers",
    "filename",
    "content_id",
    "disposition",
    "is_message",
    "encoded_size",
    "children",
}

MIXED = (
    b"From: sender@example.com\r\n"
    b"Subject: mixed\r\n"
    b'Content-Type: multipart/mixed; boundary="bnd"\r\n'
    b"\r\n"
    b"--bnd\r\n"
    b"Content-Type: text/plain\r\n"
    b"\r\n"
    b"plain version\r\n"
    b"--bnd\r\n"
    b"Content-Type: application/pdf\r\n"
    b"Content-Transfer-Encoding: base64\r\n"
    b'Content-Disposition: attachment; filename="a.pdf"\r\n'
    b"Content-ID: <pdf@example.com>\r\n"
    b"\r\n"
    b"aGVsbG8gd29ybGQ=\r\n"
    b"--bnd--\r\n"
)

SINGLE_PART = (
    b"From: sender@example.com\r\n"
    b"Subject: single\r\n"
    b"Content-Type: text/plain; charset=utf-8\r\n"
    b"Content-Transfer-Encoding: base64\r\n"
    b"\r\n"
    b"aGVsbG8gd29ybGQ=\r\n"
)

BROKEN_BASE64_BODY = (
    b"Subject: broken\r\n"
    b"Content-Type: text/plain; charset=utf-8\r\n"
    b"Content-Transfer-Encoding: base64\r\n"
    b"\r\n"
    b"!!!! not base64 !!!!\r\n"
)

MALFORMED_HEADERS = b" unexpected continuation\r\n\r\nbody"

ORIGINAL = (
    b"From: alice@example.com\r\n"
    b"To: nobody@example.invalid\r\n"
    b"Subject: the original message\r\n"
    b"Content-Type: text/plain\r\n"
    b"\r\n"
    b"hello\r\n"
)


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


def _shape(root) -> list[tuple]:
    """Every per-node field except the body, in walk order."""
    return [
        (
            part.content_type,
            part.filename,
            part.content_id,
            part.disposition,
            part.is_message,
            part.headers,
            len(part.children),
        )
        for part in walk(root)
    ]


# --- the tree is the same tree in every mode -----------------------------------


@pytest.mark.parametrize("path", FIXTURES, ids=IDS)
@pytest.mark.parametrize("mode", MODES)
def test__the_shape_is_identical_to_full_mode(path: str, mode: str):
    with open(path, "rb") as handle:
        raw = handle.read()

    assert _shape(parse_email_tree(raw, mode=mode)) == _shape(parse_email_tree(raw))


@pytest.mark.parametrize("path", FIXTURES, ids=IDS)
def test__a_leafs_bytes_are_identical_to_full_mode(path: str):
    # The load-bearing one for the lazy tree. Note it covers a case flat lazy mode
    # cannot: a single-part message's root is itself the leaf, so this asserts
    # that mailparse's `raw_bytes` is the part for a *root* too.
    with open(path, "rb") as handle:
        raw = handle.read()

    assert [part.content for part in walk(parse_email_tree(raw, mode="lazy"))] == [
        part.content for part in walk(parse_email_tree(raw))
    ]


@pytest.mark.parametrize("path", FIXTURES, ids=IDS)
def test__encoded_size_is_present_exactly_where_content_is(path: str):
    # `None` means "container" in every mode, and only that. The two modes must
    # agree with full mode about which nodes have a body of their own, or
    # `encoded_size is None` would be a different question than
    # `content is None`.
    with open(path, "rb") as handle:
        raw = handle.read()

    full = [part.content is None for part in walk(parse_email_tree(raw))]
    for mode in MODES:
        described = [
            part.encoded_size is None for part in walk(parse_email_tree(raw, mode=mode))
        ]
        assert described == full, mode


@pytest.mark.parametrize("path", FIXTURES, ids=IDS)
def test__the_two_deferred_modes_report_the_same_encoded_size(path: str):
    with open(path, "rb") as handle:
        raw = handle.read()

    assert [
        part.encoded_size for part in walk(parse_email_tree(raw, mode="lazy"))
    ] == [part.encoded_size for part in walk(parse_email_tree(raw, mode="metadata"))]


@pytest.mark.parametrize("path", FIXTURES, ids=IDS)
def test__decoding_a_leaf_does_not_more_than_double_it(path: str):
    # The bound the fuzz target settled on: quoted-printable emits a line break
    # as CRLF, so a decode can grow a body -- but not past 2x. This is what makes
    # `encoded_size` usable for "is this part worth decoding".
    with open(path, "rb") as handle:
        raw = handle.read()

    for part in walk(parse_email_tree(raw, mode="lazy")):
        if part.content is None:
            continue
        assert len(part.content) <= part.encoded_size * 2, part.content_type


def test__the_full_mode_default_is_unchanged():
    assert isinstance(parse_email_tree(MIXED), PyMimePart)
    assert isinstance(parse_email_tree(MIXED, mode="full"), PyMimePart)


def test__each_mode_returns_its_own_node_type():
    assert isinstance(parse_email_tree(MIXED, mode="lazy"), PyLazyMimePart)
    assert isinstance(parse_email_tree(MIXED, mode="metadata"), PyMimePartMetadata)


@pytest.mark.parametrize("mode", MODES)
def test__a_str_payload_gives_the_same_tree_as_its_bytes(mode: str):
    from_bytes = parse_email_tree(MIXED, mode=mode)
    from_str = parse_email_tree(MIXED.decode("ascii"), mode=mode)

    assert _shape(from_str) == _shape(from_bytes)


@pytest.mark.parametrize("mode", MODES)
def test__walk_yields_the_new_node_types(mode: str):
    parts = list(walk(parse_email_tree(_bounce(ORIGINAL), mode=mode)))

    assert [part.content_type for part in parts] == [
        "multipart/mixed",
        "text/plain",
        "message/rfc822",
        "text/plain",
    ]


# --- laziness ------------------------------------------------------------------


def test__nothing_a_leaf_carries_is_decoded_by_the_parse(large_message: str):
    root = parse_email_tree(large_message.encode(), mode="lazy")

    leaves = [part for part in walk(root) if part.encoded_size is not None]
    assert leaves, "the fixture must have leaves"
    assert not any(part.is_decoded for part in leaves)


def test__a_container_is_decoded_from_the_start(large_message: str):
    # Nothing to decode, so reading `content` is free and `is_decoded` says so.
    root = parse_email_tree(large_message.encode(), mode="lazy")

    containers = [part for part in walk(root) if part.encoded_size is None]
    assert containers, "the fixture must have containers"
    for part in containers:
        assert part.is_decoded
        assert part.content is None


def test__only_the_leaf_read_is_decoded(large_message: str):
    root = parse_email_tree(large_message.encode(), mode="lazy")
    leaves = [part for part in walk(root) if part.encoded_size is not None]
    assert len(leaves) > 1, "the fixture must have several leaves"

    assert isinstance(leaves[0].content, bytes)

    assert leaves[0].is_decoded
    assert not any(part.is_decoded for part in leaves[1:])


def test__repeated_access_returns_the_same_object():
    leaf = list(walk(parse_email_tree(MIXED, mode="lazy")))[1]

    assert leaf.content is leaf.content


def test__the_children_attribute_hands_back_the_same_objects():
    # Which is what makes the caches worth anything: a fresh object per read
    # would mean a fresh cache per read.
    root = parse_email_tree(MIXED, mode="lazy")

    assert root.children[0] is root.children[0]


def test__the_tree_outlives_the_payload():
    # The parse copies each part rather than borrowing the caller's bytes, so
    # dropping the payload cannot invalidate a deferred decode.
    payload = bytearray(MIXED)
    root = parse_email_tree(bytes(payload), mode="lazy")
    del payload

    assert list(walk(root))[1].content == b"plain version"


def test__a_single_part_message_defers_its_root():
    # The root is the leaf here, so there is no subpart to defer -- the whole
    # payload is what gets retained and re-parsed.
    root = parse_email_tree(SINGLE_PART, mode="lazy")

    assert root.children == []
    assert not root.is_decoded
    assert root.content == b"hello world"
    assert root.is_decoded


# --- embedded messages ---------------------------------------------------------


@pytest.mark.parametrize("mode", MODES)
def test__an_embedded_message_is_still_parsed_not_opaque(mode: str):
    # The tree's whole reason to exist for bounce processing, so it cannot be a
    # casualty of not decoding: a `message/rfc822` body *is* the embedded
    # message, and parsing it is what gives the node children.
    root = parse_email_tree(_bounce(ORIGINAL), mode=mode)

    embedded = [part for part in walk(root) if part.is_message]
    assert len(embedded) == 1

    inner_root = embedded[0].children[0]
    assert inner_root.headers["Subject"] == ["the original message"]
    assert inner_root.headers["From"] == ["alice@example.com"]


def test__an_embedded_message_arrives_already_decoded():
    # It had to be decoded to reach the children below it, so the bytes are
    # published rather than thrown away and decoded again.
    root = parse_email_tree(_bounce(ORIGINAL), mode="lazy")

    embedded = next(part for part in walk(root) if part.is_message)
    assert embedded.is_decoded
    assert embedded.content == parse_email_tree(_bounce(ORIGINAL)).children[1].content


def test__metadata_mode_can_raise_on_a_broken_embedded_message():
    # The documented exception to "metadata mode never decodes": it must decode a
    # `message/rfc822` body to parse the message inside it. `parse_email(...,
    # mode="metadata")` cannot raise DecodeError; this can, and says so.
    payload = (
        b"From: postmaster@example.com\r\n"
        b"Content-Type: message/rfc822\r\n"
        b"Content-Transfer-Encoding: base64\r\n"
        b"\r\n"
        b"!!!! not base64 !!!!\r\n"
    )

    with pytest.raises(DecodeError):
        parse_email_tree(payload, mode="metadata")


# --- errors --------------------------------------------------------------------


@pytest.mark.parametrize("mode", MODES)
def test__malformed_headers_still_raise(mode: str):
    with pytest.raises(HeaderParseError):
        parse_email_tree(MALFORMED_HEADERS, mode=mode)


def test__a_broken_leaf_fails_full_mode_but_not_the_deferred_modes():
    # The trade the modes make, pinned rather than left to be discovered: full
    # mode loses the whole tree to one broken part.
    with pytest.raises(DecodeError):
        parse_email_tree(BROKEN_BASE64_BODY)

    lazy = parse_email_tree(BROKEN_BASE64_BODY, mode="lazy")
    assert lazy.content_type == "text/plain"
    with pytest.raises(DecodeError):
        _ = lazy.content

    # Metadata mode never decodes a non-message leaf, so it cannot fail at all.
    described = parse_email_tree(BROKEN_BASE64_BODY, mode="metadata")
    assert described.encoded_size == len(b"!!!! not base64 !!!!\r\n")


def test__a_failed_decode_is_not_cached():
    lazy = parse_email_tree(BROKEN_BASE64_BODY, mode="lazy")

    for _ in range(2):
        with pytest.raises(DecodeError):
            _ = lazy.content
    assert not lazy.is_decoded


@pytest.mark.parametrize("mode", MODES)
def test__the_mime_depth_cap_holds(mode: str):
    payload = ORIGINAL
    for _ in range(300):
        payload = b"Content-Type: message/rfc822\r\n\r\n" + payload

    with pytest.raises(MimeStructureError):
        parse_email_tree(payload, mode=mode)


@pytest.mark.parametrize("mode", MODES)
def test__the_input_size_cap_holds(mode: str):
    oversized = b"Subject: big\r\n\r\n" + b"x" * (100 * 1024 * 1024 + 1)

    with pytest.raises(MimeStructureError):
        parse_email_tree(oversized, mode=mode)


@pytest.mark.parametrize("mode", MODES)
def test__a_non_payload_is_rejected(mode: str):
    with pytest.raises(TypeError):
        parse_email_tree(42, mode=mode)


def test__an_unknown_mode_is_rejected():
    with pytest.raises(ValueError, match='"full", "lazy" or "metadata"'):
        parse_email_tree(MIXED, mode="deferred")


def test__mode_is_keyword_only():
    with pytest.raises(TypeError):
        parse_email_tree(MIXED, "lazy")


def test__there_is_no_strict_on_the_tree():
    # The tree has no warnings channel to be strict about -- see the note on
    # `parse_email_tree`. A silently accepted `strict=` would be worse than a
    # TypeError.
    with pytest.raises(TypeError):
        parse_email_tree(MIXED, mode="lazy", strict=True)


# --- surface -------------------------------------------------------------------


def test__the_lazy_node_surface_is_frozen():
    for part in walk(parse_email_tree(MIXED, mode="lazy")):
        public = {name for name in dir(part) if not name.startswith("_")}
        assert public == EXPECTED_LAZY_NODE_ATTRS


def test__the_metadata_node_surface_is_frozen():
    for part in walk(parse_email_tree(MIXED, mode="metadata")):
        public = {name for name in dir(part) if not name.startswith("_")}
        assert public == EXPECTED_METADATA_NODE_ATTRS


def test__a_metadata_node_has_no_content_at_all():
    # Not `content = None`. `PyMimePart.content` is None for exactly one reason,
    # that the node is a container, and a mode where None also meant "not
    # decoded" would make the two indistinguishable -- the same reason
    # `PyMailMetadata` omits `text_plain` rather than returning [].
    for part in walk(parse_email_tree(MIXED, mode="metadata")):
        assert not hasattr(part, "content")


def test__the_leaf_metadata_matches_the_flat_attachment_inventory():
    # The tree and the flat projection must agree about a part's identity, not
    # just its bytes.
    described = parse_email_tree(MIXED, mode="metadata")
    pdf = next(part for part in walk(described) if part.content_type == "application/pdf")
    attachment = parse_email(MIXED, mode="metadata").attachments[0]

    assert pdf.filename == attachment.filename
    assert pdf.content_id == attachment.content_id
    assert pdf.disposition == attachment.disposition
    assert pdf.encoded_size == attachment.encoded_size


def test__repr_names_the_type_and_the_child_count():
    lazy = parse_email_tree(MIXED, mode="lazy")
    described = parse_email_tree(MIXED, mode="metadata")

    assert repr(lazy) == "<PyLazyMimePart multipart/mixed children=2 decoded=true>"
    assert repr(described) == "<PyMimePartMetadata multipart/mixed children=2>"
