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

# Same corpus and exclusion as the parity suites: invalid_message.eml is a real
# message malformed such that its structure is disputed (#150).
FIXTURES = sorted(glob.glob(os.path.join(_DATA, "rfc", "*.eml"))) + sorted(
    path
    for path in glob.glob(os.path.join(_DATA, "*.eml"))
    if os.path.basename(path) != "invalid_message.eml"
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
def test__encoded_size_is_at_least_the_decoded_size(path: str):
    # A transfer encoding never shrinks its input: base64 inflates by a third,
    # quoted-printable by however many bytes needed escaping, 7bit/8bit not at
    # all. So this holds for every encoding without knowing which was used.
    full, meta = _both(path)

    for described, decoded in zip(meta.attachments, full.attachments, strict=True):
        assert described.encoded_size >= len(decoded.content), described.filename


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
    with pytest.raises(ValueError, match="mode must be"):
        parse_email(valid_message, mode="lazy")


def test__mode_is_keyword_only(valid_message: str):
    with pytest.raises(TypeError):
        parse_email(valid_message, "metadata")
