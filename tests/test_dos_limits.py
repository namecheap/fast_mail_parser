"""DoS-hardening tests for parse_email (issue #21).

These exercise the additive guards added in src/mail_parser.rs:
  - MAX_INPUT_BYTES = 100 MiB  (oversized payload rejection)
  - MAX_MIME_DEPTH  = 256      (MIME recursion-depth cap)

Both limits sit far above any realistic email, so normal messages parse fine.
"""

import pytest

from fast_mail_parser import ParseError, parse_email


def _build_nested_multipart(levels: int) -> bytes:
    """Build a raw multipart/mixed message nested `levels` deep.

    Level 0 is the outer/root container. Each level wraps the next inside one
    `multipart/mixed` part; the innermost level carries a text/plain body. The
    parser counts the root as depth 0 and each nested subpart as +1, so the
    deepest text part sits at recursion depth `levels`.
    """
    # Innermost leaf: a simple text/plain body.
    inner = (
        b"Content-Type: text/plain\r\n\r\n"
        b"deeply nested body\r\n"
    )

    body = inner
    for level in range(levels):
        boundary = f"b{level}".encode("ascii")
        body = (
            b"Content-Type: multipart/mixed; boundary=\"" + boundary + b"\"\r\n\r\n"
            b"--" + boundary + b"\r\n"
            + body
            + b"\r\n--" + boundary + b"--\r\n"
        )

    # Add a top-level header block so the whole thing looks like an email.
    return b"Subject: nested\r\n" + body


def test_normal_message_parses():
    """A normal, shallow message parses without error (no regression)."""
    message = (
        b"Subject: hello\r\n"
        b"Content-Type: text/plain\r\n\r\n"
        b"plain body\r\n"
    )
    mail = parse_email(message)
    assert mail.subject == "hello"
    assert any("plain body" in part for part in mail.text_plain)


def test_normally_nested_multipart_parses():
    """Multipart nested well within MAX_MIME_DEPTH parses fine."""
    message = _build_nested_multipart(10)
    mail = parse_email(message)
    assert mail.subject == "nested"
    assert any("deeply nested body" in part for part in mail.text_plain)


def test_deeply_nested_multipart_rejected():
    """Nesting beyond MAX_MIME_DEPTH (256) raises ParseError, not a crash."""
    # 300 levels is comfortably past the 256-level cap.
    message = _build_nested_multipart(300)
    with pytest.raises(ParseError):
        parse_email(message)


def test_oversized_input_rejected():
    """A payload just over MAX_INPUT_BYTES (100 MiB) raises ParseError.

    Allocating ~100 MiB once in-process is acceptable for a single test run;
    the cap itself is intentionally kept generous so production emails are
    never affected. This asserts the guard fires before any parsing work.
    """
    max_input_bytes = 100 * 1024 * 1024
    # A header line plus filler to push total length just past the cap.
    oversized = b"Subject: big\r\n\r\n" + b"x" * (max_input_bytes + 1)
    with pytest.raises(ParseError):
        parse_email(oversized)
