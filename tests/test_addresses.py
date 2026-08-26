"""Tests for the typed address fields (from_/to/cc/bcc/reply_to).

Each is a list of ``PyAddress`` (``from_`` is a single value or ``None``) parsed
from the first occurrence of its header.

Contract:
- RFC 5322 groups are flattened to their member mailboxes; the group name is
  structure and is not exposed.
- RFC 2047 encoded-words in display names are decoded, including inside a quoted
  name.
- A display name is ``None`` when the header carries a bare address.
- An address header that does not parse yields an empty list (or ``None`` for
  ``from_``) rather than raising. mailparse rejects an address with no ``@``, so
  that is the malformed case exercised here. The raw value stays in ``headers``.
"""

import pytest

from fast_mail_parser import PyMail, parse_email


def _parse(header_line: str) -> PyMail:
    raw = f"Subject: addresses\r\n{header_line}\r\n\r\nbody\r\n"
    return parse_email(raw.encode("utf-8"))


# header value -> expected [(display_name, address), ...] for `To`
CASES = {
    "bare address": (
        "a@example.com",
        [(None, "a@example.com")],
    ),
    "display name": (
        "foo bar <foo@bar.com>",
        [("foo bar", "foo@bar.com")],
    ),
    "quoted display name containing a comma": (
        '"Doe, Jane" <jane@example.com>',
        [("Doe, Jane", "jane@example.com")],
    ),
    "multiple recipients, mixed forms": (
        "foo <ba@r.com>, jo@e.com, baz <qu@ux.com>",
        [("foo", "ba@r.com"), (None, "jo@e.com"), ("baz", "qu@ux.com")],
    ),
    "group is flattened to its members": (
        "bar-group: foo <foo@bar.com>, baz@bar.com;",
        [("foo", "foo@bar.com"), (None, "baz@bar.com")],
    ),
    "single alongside a group": (
        "joe@bloe.com, bar-group: foo <foo@bar.com>;",
        [(None, "joe@bloe.com"), ("foo", "foo@bar.com")],
    ),
    "empty group yields nothing": (
        "empty-group:;",
        [],
    ),
    "rfc 2047 encoded display name": (
        "=?UTF-8?B?0JjQvNGPLCDQpNCw0LzQuNC70LjRjw==?= <foobar@example.com>",
        [("Имя, Фамилия", "foobar@example.com")],
    ),
    "rfc 2047 inside a quoted display name": (
        '"=?utf-8?q?G=C3=B6tz?= C" <g@c.de>',
        [("Götz C", "g@c.de")],
    ),
    "malformed address without @ yields nothing": (
        "not-an-address",
        [],
    ),
}


@pytest.mark.parametrize("name", list(CASES), ids=list(CASES))
def test__to_header_parses_to_expected_mailboxes(name: str):
    value, expected = CASES[name]
    mail = _parse(f"To: {value}")

    assert [(a.display_name, a.address) for a in mail.to] == expected


def test__malformed_header_still_available_raw():
    # Never an exception for a bad To: in an otherwise good mail -- and the raw
    # value is not lost, just unparsed.
    mail = _parse("To: not-an-address")

    assert mail.to == []
    assert mail.headers["To"] == ["not-an-address"]
    assert mail.subject == "addresses"  # the rest of the message parsed fine


def test__from_is_a_single_address():
    mail = _parse("From: Jane Doe <jane@example.com>")

    assert mail.from_ is not None
    assert mail.from_.display_name == "Jane Doe"
    assert mail.from_.address == "jane@example.com"


def test__from_is_none_when_absent_or_unparseable():
    assert _parse("Subject: x").from_ is None
    assert _parse("From: nonsense").from_ is None


def test__cc_bcc_and_reply_to_are_parsed():
    mail = _parse(
        "Cc: c@example.com\r\n"
        "Bcc: Blind <b@example.com>\r\n"
        "Reply-To: reply-group: r1@example.com, r2@example.com;"
    )

    assert [a.address for a in mail.cc] == ["c@example.com"]
    assert [(a.display_name, a.address) for a in mail.bcc] == [("Blind", "b@example.com")]
    assert [a.address for a in mail.reply_to] == ["r1@example.com", "r2@example.com"]


def test__absent_address_headers_are_empty_lists():
    mail = _parse("Subject: nothing else")

    assert mail.to == []
    assert mail.cc == []
    assert mail.bcc == []
    assert mail.reply_to == []
    assert mail.from_ is None
