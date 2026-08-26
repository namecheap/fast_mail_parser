"""Characterization tests over an RFC-feature .eml corpus.

Each fixture in tests/data/rfc/ exercises a specific email/MIME RFC feature.
These tests pin fast_mail_parser's *actual* output per feature so that any drift
— across releases or native-binding upgrades — is caught. The fixtures are
generated deterministically by tests/generate_rfc_corpus.py.

Behaviors intentionally locked here (current contract):
- `attachments` holds only real attachments. `multipart/*` container nodes are
  MIME structure and are not reported; body parts belong to `text_plain` /
  `text_html`, not to `attachments`.
- RFC 2183 decides body-vs-attachment: a part is body text when it is
  `text/plain` or `text/html` and is not marked `Content-Disposition:
  attachment`. A `Content-Type; name` parameter alone does not demote a body.
- `filename` prefers the Content-Disposition `filename` parameter (including
  RFC 2231 extended values) and falls back to Content-Type `name`.
- str input is decoded lossily (code point -> low byte), so arbitrary `.eml`
  files (which may contain raw UTF-8 under 8BITMIME/SMTPUTF8) are parsed from
  bytes here.
"""

import glob
import os
import re

import pytest

from fast_mail_parser import PyAttachment, PyMail, parse_email

RFC_DIR = os.path.join(os.path.dirname(__file__), "data", "rfc")
ALL_FIXTURES = sorted(glob.glob(os.path.join(RFC_DIR, "*.eml")))

FOLDED_SUBJECT = (
    "This is a deliberately long subject line that the email generator must "
    "fold across multiple physical lines using folding whitespace per RFC 5322 "
    "so the parser has to unfold it back into a single logical value"
)

# fixture name -> expected shape:
#   (subject, n_text_plain, n_text_html, n_attachments, ordered attachment mimetypes)
CASES = {
    "rfc5322_plain": (
        "Plain text message", 1, 0, 0, [],
    ),
    "multipart_alternative": (
        "Alternative parts", 1, 1, 0, [],
    ),
    "multipart_mixed_attachment": (
        "Message with attachment", 1, 0, 1, ["image/png"],
    ),
    "base64_body": (
        "Base64 body", 1, 0, 0, [],
    ),
    "quoted_printable_body": (
        "Quoted-printable body", 1, 0, 0, [],
    ),
    "rfc2047_encoded_subject": (
        "Café ☕ — déjà vu update", 1, 0, 0, [],
    ),
    "rfc2231_param_filename": (
        "Attachment with encoded filename", 1, 0, 1, ["application/pdf"],
    ),
    "multipart_related": (
        "Related inline image", 0, 1, 1, ["image/png"],
    ),
    "nested_multipart": (
        "Nested multipart", 1, 1, 1, ["application/pdf"],
    ),
    "rfc6532_utf8_headers": (
        "Письмо с UTF-8 заголовками", 1, 0, 0, [],
    ),
    "utf8_8bit_body": (
        "8bit UTF-8 body", 1, 0, 0, [],
    ),
    "empty_body": (
        "No body", 1, 0, 0, [],
    ),
    "folded_header": (
        FOLDED_SUBJECT, 1, 0, 0, [],
    ),
    "inline_text_with_name_param": (
        "Inline text with name param", 1, 0, 0, [],
    ),
    "disposition_only_text_attachment": (
        "Text attachment via disposition", 1, 0, 1, ["text/plain"],
    ),
}


def _load(name: str) -> PyMail:
    with open(os.path.join(RFC_DIR, f"{name}.eml"), "rb") as handle:
        return parse_email(handle.read())


def test__corpus_and_cases_are_in_sync():
    # Adding a fixture without an expected-shape entry (or vice versa) fails here.
    on_disk = {os.path.splitext(os.path.basename(p))[0] for p in ALL_FIXTURES}
    assert on_disk == set(CASES), f"corpus/CASES mismatch: {on_disk ^ set(CASES)}"


@pytest.mark.parametrize("path", ALL_FIXTURES, ids=lambda p: os.path.basename(p))
def test__every_fixture_parses_to_valid_pymail(path: str):
    with open(path, "rb") as handle:
        mail = parse_email(handle.read())

    assert isinstance(mail, PyMail)
    assert isinstance(mail.subject, str)
    assert isinstance(mail.headers, dict) and mail.headers
    for attachment in mail.attachments:
        assert isinstance(attachment, PyAttachment)
        assert isinstance(attachment.mimetype, str)
        assert isinstance(attachment.filename, str)
        assert isinstance(attachment.content, bytes)


@pytest.mark.parametrize("name", sorted(CASES))
def test__fixture_structure_matches_expected(name: str):
    subject, n_plain, n_html, n_attachments, mimetypes = CASES[name]
    mail = _load(name)

    assert mail.subject == subject
    assert len(mail.text_plain) == n_plain
    assert len(mail.text_html) == n_html
    assert len(mail.attachments) == n_attachments
    assert [a.mimetype for a in mail.attachments] == mimetypes


# --- feature-specific behavior locks ----------------------------------------


def test__rfc2047_encoded_word_subject_is_decoded():
    mail = _load("rfc2047_encoded_subject")
    assert mail.subject == "Café ☕ — déjà vu update"
    assert "=?" not in mail.subject  # decoded, not the raw encoded-word


def test__rfc6532_raw_utf8_header_is_decoded():
    mail = _load("rfc6532_utf8_headers")
    assert mail.subject == "Письмо с UTF-8 заголовками"


def test__base64_transfer_encoding_is_decoded():
    mail = _load("base64_body")
    assert "transferred as base64" in mail.text_plain[0]


def test__quoted_printable_transfer_encoding_is_decoded():
    mail = _load("quoted_printable_body")
    assert "café" in mail.text_plain[0]


def test__8bit_utf8_body_is_decoded():
    mail = _load("utf8_8bit_body")
    assert "日本語" in mail.text_plain[0]


def test__folded_header_is_unfolded_to_single_value():
    mail = _load("folded_header")
    assert mail.subject == FOLDED_SUBJECT


def test__binary_attachment_survives_base64_round_trip():
    mail = _load("multipart_mixed_attachment")
    png = next(a for a in mail.attachments if a.mimetype == "image/png")
    assert png.content[:8] == b"\x89PNG\r\n\x1a\n"  # PNG magic intact after decode


def test__filename_is_read_from_disposition_and_rfc2231_decoded():
    # add_attachment sets the filename only in Content-Disposition, here as an
    # RFC 2231 extended value (filename*=utf-8''...). Both the disposition
    # lookup and the 2231 decoding must apply.
    mail = _load("rfc2231_param_filename")
    pdf = next(a for a in mail.attachments if a.mimetype == "application/pdf")
    assert pdf.filename == "résumé déjà.pdf"


def test__multipart_container_nodes_are_not_reported():
    # Container nodes are MIME structure, not content. Reporting them produced
    # phantom, filename-less attachment entries.
    for name in ("nested_multipart", "multipart_alternative", "multipart_related"):
        mail = _load(name)
        assert not [a for a in mail.attachments if a.mimetype.startswith("multipart/")], (
            f"{name}: multipart containers must not appear in attachments"
        )


def test__body_parts_are_not_reported_as_attachments():
    # A plain single-part message has a body and no attachments at all.
    mail = _load("rfc5322_plain")
    assert len(mail.text_plain) == 1
    assert mail.attachments == []

    # An alternative pair yields two bodies and still no attachments.
    mail = _load("multipart_alternative")
    assert (len(mail.text_plain), len(mail.text_html)) == (1, 1)
    assert mail.attachments == []


def test__content_type_name_alone_does_not_demote_a_body():
    # RFC 2183: only an `attachment` disposition makes a part a file. A
    # Content-Type `name` param on an inline text part must not remove it from
    # the body -- that silently lost body text.
    mail = _load("inline_text_with_name_param")
    assert len(mail.text_plain) == 1
    assert "still body text" in mail.text_plain[0]
    assert mail.attachments == []


def test__text_attachment_by_disposition_is_not_body():
    # A text/plain part marked `Content-Disposition: attachment` is a file. Its
    # bytes must not be concatenated into the body.
    mail = _load("disposition_only_text_attachment")

    assert len(mail.text_plain) == 1
    assert "The real body." in mail.text_plain[0]
    assert "log line one" not in mail.text_plain[0]

    attachment = next(a for a in mail.attachments if a.filename == "log.txt")
    assert attachment.mimetype == "text/plain"
    assert b"log line one" in attachment.content


# --- Content-ID / inline resolution ----------------------------------------


def test__content_id_is_normalized_without_angle_brackets():
    # The fixture declares `Content-ID: <img1>`; RFC 2392 cid: URLs reference the
    # bracket-less form, so that is what is exposed.
    mail = _load("multipart_related")
    png = next(a for a in mail.attachments if a.mimetype == "image/png")

    assert png.content_id == "img1"


def test__disposition_reports_the_raw_token():
    mail = _load("multipart_related")
    png = next(a for a in mail.attachments if a.mimetype == "image/png")
    assert png.disposition == "inline"

    # add_attachment marks parts `attachment`.
    mail = _load("multipart_mixed_attachment")
    png = next(a for a in mail.attachments if a.mimetype == "image/png")
    assert png.disposition == "attachment"


def test__cid_references_in_html_resolve_to_attachments():
    # The canonical HTML-mail task, end to end: map every cid: URL in the body
    # to the attachment that carries it.
    mail = _load("multipart_related")

    by_cid = {a.content_id: a for a in mail.attachments if a.content_id}
    referenced = re.findall(r'cid:([^"\'>\s]+)', mail.text_html[0])

    assert referenced, "fixture must reference at least one cid:"
    for cid in referenced:
        assert cid in by_cid, f"unresolved cid reference: {cid}"
        assert by_cid[cid].content, f"cid {cid} resolved to an empty part"

    assert referenced == ["img1"]
    assert by_cid["img1"].content[:8] == b"\x89PNG\r\n\x1a\n"
