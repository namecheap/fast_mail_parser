"""Body-decode error tests for parse_email (issue #24).

Previously `src/mail_parser.rs` decoded part bodies with
`get_body_raw().unwrap_or_default()` / `get_body().unwrap_or_default()`, which
silently turned a failed transfer-encoding decode (e.g. invalid base64) into an
empty body. That hid corruption from the caller.

The fix propagates those `MailParseError`s through `Mail::new`; the PyO3 layer
maps them to `ParseError`. These tests assert broken encodings now surface as
`ParseError` while valid messages (including valid base64) still parse.
"""

import base64

import pytest

from fast_mail_parser import ParseError, parse_email


def test_broken_base64_text_body_raises():
    """A text/plain body declaring base64 with invalid base64 raises ParseError.

    This exercises the `get_body()` path. Before the fix the body was silently
    stored as empty; now the decode error is surfaced.
    """
    message = (
        b"Subject: broken\r\n"
        b"Content-Type: text/plain\r\n"
        b"Content-Transfer-Encoding: base64\r\n\r\n"
        b"!!!!not_valid_base64!!!!\r\n"
    )
    with pytest.raises(ParseError):
        parse_email(message)


def test_broken_base64_attachment_raises():
    """A named attachment with invalid base64 raises ParseError.

    This exercises the `get_body_raw()` path used for every part's content.
    """
    message = (
        b"Subject: bad-attach\r\n"
        b'Content-Type: application/octet-stream; name="x.bin"\r\n'
        b"Content-Transfer-Encoding: base64\r\n\r\n"
        b"@@@not-base64@@@\r\n"
    )
    with pytest.raises(ParseError):
        parse_email(message)


def test_valid_base64_body_still_parses():
    """A valid base64-encoded text body still decodes (no regression)."""
    encoded = base64.b64encode(b"hello base64 body").decode("ascii")
    message = (
        b"Subject: ok\r\n"
        b"Content-Type: text/plain\r\n"
        b"Content-Transfer-Encoding: base64\r\n\r\n"
        + encoded.encode("ascii")
        + b"\r\n"
    )
    mail = parse_email(message)
    assert mail.subject == "ok"
    assert any("hello base64 body" in part for part in mail.text_plain)


def test_plain_message_still_parses():
    """A normal, unencoded message still parses unaffected (no regression)."""
    message = (
        b"Subject: hello\r\n"
        b"Content-Type: text/plain\r\n\r\n"
        b"plain body\r\n"
    )
    mail = parse_email(message)
    assert mail.subject == "hello"
    assert any("plain body" in part for part in mail.text_plain)
