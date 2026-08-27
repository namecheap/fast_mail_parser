"""`parse_email(payload, mode="metadata")` (#97).

Metadata mode reads the headers and the attachment inventory and decodes nothing.
The contract that matters is that what it *does* report is identical to full mode
-- otherwise a triage sweep and a full parse would disagree about the same
message -- so most of this file is equality against full mode over the corpus.
"""
import glob
import os

import pytest

from fast_mail_parser import (
    HeaderParseError,
    PyAttachmentMetadata,
    PyMail,
    PyMailMetadata,
    parse_email,
)

_DATA = os.path.join(os.path.dirname(__file__), "data")

# Every fixture, invalid_message.eml included. It was excluded while its structure
# was disputed (#150); now that the missing header/body separator is repaired, the
# two modes must agree about it like any other message -- and this test would have
# caught the fact that the repair initially missed metadata mode, which the
# exclusion is exactly why it did not.
FIXTURES = sorted(glob.glob(os.path.join(_DATA, "rfc", "*.eml"))) + sorted(
    glob.glob(os.path.join(_DATA, "*.eml"))
)
IDS = [os.path.basename(path) for path in FIXTURES]


def _both(path: str) -> tuple[PyMail, PyMailMetadata]:
    with open(path, "rb") as handle:
        raw = handle.read()
    return parse_email(raw), parse_email(raw, mode="metadata")


# --- what metadata mode reports must match full mode --------------------------


@pytest.mark.parametrize("path", FIXTURES, ids=IDS)
def test__the_envelope_is_identical_to_full_mode(path: str):
    full, meta = _both(path)

    assert meta.subject == full.subject
    assert meta.date == full.date
    assert meta.headers == full.headers
    assert meta.date_parsed == full.date_parsed


@pytest.mark.parametrize("path", FIXTURES, ids=IDS)
def test__addresses_are_identical_to_full_mode(path: str):
    full, meta = _both(path)

    def flatten(addresses):
        return [(a.display_name, a.address) for a in addresses]

    assert (meta.from_ is None) == (full.from_ is None)
    if full.from_ is not None:
        assert (meta.from_.display_name, meta.from_.address) == (
            full.from_.display_name,
            full.from_.address,
        )
    assert flatten(meta.to) == flatten(full.to)
    assert flatten(meta.cc) == flatten(full.cc)
    assert flatten(meta.bcc) == flatten(full.bcc)
    assert flatten(meta.reply_to) == flatten(full.reply_to)


@pytest.mark.parametrize("path", FIXTURES, ids=IDS)
def test__the_attachment_inventory_is_identical_to_full_mode(path: str):
    full, meta = _both(path)

    assert len(meta.attachments) == len(full.attachments)
    for described, decoded in zip(meta.attachments, full.attachments, strict=True):
        assert isinstance(described, PyAttachmentMetadata)
        assert described.mimetype == decoded.mimetype
        assert described.filename == decoded.filename
        assert described.content_id == decoded.content_id
        assert described.disposition == decoded.disposition


@pytest.mark.parametrize("path", FIXTURES, ids=IDS)
def test__decoding_does_not_amplify_a_part(path: str):
    # NOT "encoded is at least decoded", which this asserted until a fuzz run
    # produced 45 counterexamples: quoted-printable emits a line break as CRLF,
    # so a body of bare LFs decodes larger than it was encoded. Doubling is the
    # true bound -- see test__quoted_printable_can_decode_larger_than_its_encoded_size.
    full, meta = _both(path)

    for described, decoded in zip(meta.attachments, full.attachments, strict=True):
        assert len(decoded.content) <= described.encoded_size * 2, described.filename


def test__quoted_printable_can_decode_larger_than_its_encoded_size():
    # `encoded_size` is the bytes on the wire, and it is NOT an upper bound on the
    # decoded size. The `quoted_printable` crate emits a line break as CRLF, so a
    # body of bare LFs gains a byte per line.
    #
    # Found by the `parse_agreement` fuzz target, which produced 45 crashers in
    # fifteen minutes -- every one quoted-printable -- against an invariant that
    # said this could not happen. Pinned here so the surprise is documented where
    # someone reading the attribute will meet it.
    message = (
        b"Subject: qp\r\n"
        b'Content-Type: application/octet-stream; name="x"\r\n'
        b"Content-Disposition: attachment\r\n"
        b"Content-Transfer-Encoding: quoted-printable\r\n"
        b"\r\n"
        b"a\nb\nc\n"
    )

    full = parse_email(message)
    meta = parse_email(message, mode="metadata")

    described = meta.attachments[0]
    decoded = full.attachments[0].content

    assert described.encoded_size < len(decoded), (
        f"expected quoted-printable to grow: encoded {described.encoded_size}, "
        f"decoded {len(decoded)}"
    )
    # Doubling is the real bound, and what the fuzz target now asserts.
    assert len(decoded) <= described.encoded_size * 2


def test__encoded_size_is_larger_for_base64(attachment_message: str):
    # Concrete rather than only invariant: a base64 part must report more wire
    # bytes than decoded bytes.
    full = parse_email(attachment_message)
    meta = parse_email(attachment_message, mode="metadata")

    base64_parts = [
        (described, decoded)
        for described, decoded in zip(meta.attachments, full.attachments, strict=True)
        if described.encoded_size > len(decoded.content)
    ]

    assert base64_parts, "expected at least one non-identity transfer encoding"


# --- what metadata mode deliberately does not report --------------------------


def test__bodies_are_absent_not_empty(valid_message: str):
    # An empty list would be indistinguishable from "this message has no text
    # part", so a sweep counting bodyless messages would count every one. The
    # attribute is absent instead, which fails loudly.
    meta = parse_email(valid_message, mode="metadata")

    assert not hasattr(meta, "text_plain")
    assert not hasattr(meta, "text_html")


def test__described_attachments_carry_no_content(attachment_message: str):
    meta = parse_email(attachment_message, mode="metadata")

    assert meta.attachments
    for described in meta.attachments:
        assert not hasattr(described, "content")


def test__metadata_mode_cannot_report_a_decode_error():
    # It does not decode, so a broken transfer encoding is invisible to it. A
    # real difference in what the two modes can tell you, worth pinning rather
    # than discovering.
    broken = (
        b"Subject: broken\r\n"
        b"Content-Type: text/plain; charset=utf-8\r\n"
        b"Content-Transfer-Encoding: base64\r\n"
        b"\r\n"
        b"!!!! not base64 !!!!\r\n"
    )

    meta = parse_email(broken, mode="metadata")

    assert meta.subject == "broken"


def test__header_errors_are_still_reported():
    # Header parsing happens in both modes, so this failure is common to both.
    with pytest.raises(HeaderParseError):
        parse_email(b" unexpected continuation\r\n\r\nbody", mode="metadata")


# --- the mode argument itself -------------------------------------------------


def test__explicit_full_mode_matches_the_default(valid_message: str):
    default = parse_email(valid_message)
    explicit = parse_email(valid_message, mode="full")

    assert isinstance(default, PyMail)
    assert isinstance(explicit, PyMail)
    assert explicit.subject == default.subject
    assert explicit.text_plain == default.text_plain


def test__an_unknown_mode_is_rejected(valid_message: str):
    # Not `"lazy"`, which this asserted until lazy mode landed (#97).
    with pytest.raises(ValueError, match="mode must be"):
        parse_email(valid_message, mode="headers")


def test__mode_is_keyword_only(valid_message: str):
    with pytest.raises(TypeError):
        parse_email(valid_message, "metadata")
