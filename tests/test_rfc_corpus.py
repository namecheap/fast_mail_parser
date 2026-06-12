"""Characterization tests over an RFC-feature .eml corpus.

Each fixture in tests/data/rfc/ exercises a specific email/MIME RFC feature.
These tests pin fast_mail_parser's *actual* output per feature so that any drift
— across releases or native-binding upgrades — is caught. The fixtures are
generated deterministically by tests/generate_rfc_corpus.py.

Behaviors intentionally locked here (current contract, including quirks):
- Every node of the MIME tree is reported as an attachment — text parts and the
  multipart *containers* themselves (containers carry empty content).
- `filename` is read from the Content-Type `name` parameter, not from
  Content-Disposition `filename`; attachments declared only via
  Content-Disposition therefore report an empty filename.
- str input is decoded lossily (code point -> low byte), so arbitrary `.eml`
  files (which may contain raw UTF-8 under 8BITMIME/SMTPUTF8) are parsed from
  bytes here.
"""

import glob
import os

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
        "Plain text message", 1, 0, 1, ["text/plain"],
    ),
    "multipart_alternative": (
        "Alternative parts", 1, 1, 3,
        ["text/plain", "text/html", "multipart/alternative"],
    ),
    "multipart_mixed_attachment": (
        "Message with attachment", 1, 0, 3,
        ["text/plain", "image/png", "multipart/mixed"],
    ),
    "base64_body": (
        "Base64 body", 1, 0, 1, ["text/plain"],
    ),
    "quoted_printable_body": (
        "Quoted-printable body", 1, 0, 1, ["text/plain"],
    ),
    "rfc2047_encoded_subject": (
        "Café ☕ — déjà vu update", 1, 0, 1, ["text/plain"],
    ),
    "rfc2231_param_filename": (
        "Attachment with encoded filename", 1, 0, 3,
        ["text/plain", "application/pdf", "multipart/mixed"],
    ),
    "multipart_related": (
        "Related inline image", 0, 1, 3,
        ["text/html", "image/png", "multipart/related"],
    ),
    "nested_multipart": (
        "Nested multipart", 1, 1, 5,
        ["text/plain", "text/html", "multipart/alternative",
         "application/pdf", "multipart/mixed"],
    ),
    "rfc6532_utf8_headers": (
        "Письмо с UTF-8 заголовками", 1, 0, 1, ["text/plain"],
    ),
    "utf8_8bit_body": (
        "8bit UTF-8 body", 1, 0, 1, ["text/plain"],
    ),
    "empty_body": (
        "No body", 1, 0, 1, ["text/plain"],
    ),
    "folded_header": (
        FOLDED_SUBJECT, 1, 0, 1, ["text/plain"],
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


def test__filename_is_read_from_content_type_name_not_disposition():
    # add_attachment sets only Content-Disposition filename; the parser reads the
    # Content-Type `name` param, so these attachments report empty filenames.
    mail = _load("rfc2231_param_filename")
    pdf = next(a for a in mail.attachments if a.mimetype == "application/pdf")
    assert pdf.filename == ""


def test__multipart_container_nodes_are_reported_with_empty_content():
    mail = _load("nested_multipart")
    containers = [a for a in mail.attachments if a.mimetype.startswith("multipart/")]
    assert containers, "expected the multipart container nodes to appear"
    assert all(a.content == b"" for a in containers)
