"""Tests for the ParseError hierarchy.

Failures are categorised at the point they occur, so a caller can tell "this is
not an email" from "one attachment's base64 is broken" -- distinctions worth
acting on differently in a mail pipeline.

Every subtype inherits from `ParseError`, so code written against the old single
exception keeps working; that compatibility is asserted here rather than assumed.
"""

import pytest

from fast_mail_parser import (
    DecodeError,
    HeaderParseError,
    MimeStructureError,
    ParseError,
    parse_email,
)

SUBTYPES = (HeaderParseError, MimeStructureError, DecodeError)

# A leading whitespace line is an overhanging continuation with no preceding
# header key, which mailparse rejects outright.
MALFORMED_HEADERS = b" unexpected continuation\r\n\r\nbody"

BROKEN_BASE64_BODY = (
    b"Subject: broken\r\n"
    b"Content-Type: text/plain; charset=utf-8\r\n"
    b"Content-Transfer-Encoding: base64\r\n"
    b"\r\n"
    b"!!!! not base64 !!!!\r\n"
)


# The two MimeStructureError paths -- the 100 MiB input cap and the 256-level
# nesting cap -- are asserted in tests/test_dos_limits.py, which already builds
# those payloads. Rebuilding a 100 MiB buffer here purely to re-check the
# exception type would double that cost for nothing.


# --- the hierarchy itself ----------------------------------------------------


@pytest.mark.parametrize("subtype", SUBTYPES, ids=lambda t: t.__name__)
def test__every_subtype_inherits_parse_error(subtype: type):
    assert issubclass(subtype, ParseError)


@pytest.mark.parametrize("subtype", SUBTYPES, ids=lambda t: t.__name__)
def test__subtypes_are_distinct(subtype: type):
    others = [other for other in SUBTYPES if other is not subtype]
    for other in others:
        assert not issubclass(subtype, other), f"{subtype} must not be a {other}"


# --- which failure raises which -----------------------------------------------


def test__malformed_headers_raise_header_parse_error():
    with pytest.raises(HeaderParseError):
        parse_email(MALFORMED_HEADERS)


def test__broken_transfer_encoding_raises_decode_error():
    with pytest.raises(DecodeError):
        parse_email(BROKEN_BASE64_BODY)


# --- backwards compatibility --------------------------------------------------


@pytest.mark.parametrize(
    "payload",
    [MALFORMED_HEADERS, BROKEN_BASE64_BODY],
    ids=["malformed_headers", "broken_base64"],
)
def test__catching_parse_error_still_catches_everything(payload: bytes):
    # The contract existing callers were written against.
    with pytest.raises(ParseError):
        parse_email(payload)


def test__a_decode_error_is_not_a_header_error():
    # The point of the taxonomy: these two are told apart, not merged.
    with pytest.raises(DecodeError):
        parse_email(BROKEN_BASE64_BODY)

    try:
        parse_email(BROKEN_BASE64_BODY)
    except ParseError as exc:
        assert not isinstance(exc, HeaderParseError)
        assert isinstance(exc, DecodeError)


def test__messages_carry_the_underlying_detail():
    try:
        parse_email(BROKEN_BASE64_BODY)
    except DecodeError as exc:
        assert "Message parsing error" in str(exc)
        assert str(exc) != "Message parsing error: "
