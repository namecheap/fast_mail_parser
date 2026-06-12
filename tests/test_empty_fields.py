"""Tests for the empty-string sentinel on missing Subject/Date headers.

Contract documented and exercised here:

When a parsed message lacks a ``Subject`` or ``Date`` header, the parser does
NOT raise and does NOT return ``None``. Instead each missing field is reported
as the empty string ``""``.

This mirrors the Rust implementation in ``src/mail_parser.rs``, which reads::

    headers.get("Subject").map(String::from).unwrap_or_default()
    headers.get("Date").map(String::from).unwrap_or_default()

``unwrap_or_default()`` on a ``String`` yields ``""``. Callers must therefore
treat ``""`` as "header absent" and cannot distinguish a missing header from a
header whose value is genuinely empty.

These tests build messages inline (no fixtures) so the header presence/absence
under test is unambiguous.
"""

from fast_mail_parser import parse_email


def test__missing_subject_yields_empty_string():
    mail = parse_email(b"From: a@b.com\r\nTo: c@d.com\r\n\r\nbody\r\n")

    assert mail.subject == ""


def test__missing_date_yields_empty_string():
    mail = parse_email(b"From: a@b.com\r\nTo: c@d.com\r\n\r\nbody\r\n")

    assert mail.date == ""


def test__subject_present_but_date_missing():
    mail = parse_email(b"Subject: Hello there\r\nFrom: a@b.com\r\n\r\nbody\r\n")

    assert mail.subject == "Hello there"
    assert mail.date == ""


def test__date_present_but_subject_missing():
    mail = parse_email(
        b"Date: Wed, 1 Jul 2020 05:33:42 +0000\r\nFrom: a@b.com\r\n\r\nbody\r\n"
    )

    assert mail.date == "Wed, 1 Jul 2020 05:33:42 +0000"
    assert mail.subject == ""


def test__both_subject_and_date_missing():
    mail = parse_email(b"From: a@b.com\r\nTo: c@d.com\r\n\r\nbody\r\n")

    assert mail.subject == ""
    assert mail.date == ""


def test__missing_headers_return_str_not_none():
    # The sentinel is the empty *string*, never None, so callers can rely on
    # str operations without a None check.
    mail = parse_email(b"From: a@b.com\r\n\r\nbody\r\n")

    assert isinstance(mail.subject, str)
    assert isinstance(mail.date, str)


def test__header_only_message_has_no_html_part():
    # A message with no HTML body part exposes an empty text_html list. (Note:
    # a header-only message still surfaces a default text/plain part, so
    # text_plain and attachments are intentionally not asserted empty here.)
    mail = parse_email(b"Subject: only headers\r\nFrom: a@b.com\r\n\r\n")

    assert mail.text_html == []
