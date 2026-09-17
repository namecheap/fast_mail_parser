"""Charset decoding of already-owned bodies (#230).

`decode_charset_owned` takes a shortcut for the common case -- a UTF-8-labelled
body with no BOM goes through `String::from_utf8` instead of encoding_rs -- and
routes everything else to the original path. These pin the two inputs where the
shortcut must NOT be taken, because encoding_rs does something `from_utf8` does
not: strip a BOM, and replace invalid sequences with U+FFFD.

All of these pass on master too. That is the point: they are the oracle for the
claim that the shortcut changed nothing.
"""
import base64
import quopri

import pytest

from fast_mail_parser import parse_email


def _message(body: bytes, encoding: str, charset: str = "utf-8") -> bytes:
    if encoding == "base64":
        payload = base64.b64encode(body)
    elif encoding == "quoted-printable":
        payload = quopri.encodestring(body)
    else:
        payload = body
    return (
        f"Subject: decode\r\n"
        f"Content-Type: text/plain; charset={charset}\r\n"
        f"Content-Transfer-Encoding: {encoding}\r\n\r\n"
    ).encode() + payload + b"\r\n"


@pytest.mark.parametrize("encoding", ["base64", "quoted-printable", "7bit", "8bit"])
def test__a_utf8_bom_is_stripped_whatever_the_transfer_encoding(encoding: str):
    # encoding_rs strips a leading BOM; String::from_utf8 would keep it as
    # U+FEFF. The fast path must therefore not be taken for a BOM-prefixed body.
    mail = parse_email(_message("﻿hello".encode(), encoding))

    assert [t.strip() for t in mail.text_plain] == ["hello"], "the BOM must not survive"


@pytest.mark.parametrize("encoding", ["base64", "quoted-printable"])
def test__invalid_utf8_is_replaced_not_rejected(encoding: str):
    # encoding_rs replaces an invalid sequence with U+FFFD. String::from_utf8
    # would fail, so the fast path has to fall back rather than propagate.
    mail = parse_email(_message(b"caf\xe9 bar", encoding))

    assert mail.text_plain, "an undecodable byte must not empty the body"
    assert "�" in mail.text_plain[0]
    assert mail.text_plain[0].strip().endswith(" bar")


@pytest.mark.parametrize("encoding", ["base64", "quoted-printable", "7bit", "8bit"])
def test__plain_utf8_round_trips(encoding: str):
    mail = parse_email(_message("café ☕ déjà".encode(), encoding))

    assert [t.strip() for t in mail.text_plain] == ["café ☕ déjà"]


@pytest.mark.parametrize("label", ["utf8", "UTF-8", "unicode-1-1-utf-8", "UTF8"])
def test__every_spelling_of_utf8_takes_the_same_route(label: str):
    # `Charset::for_label` maps all of these onto one charset, which is why the
    # resolved charset is compared and never the raw label.
    mail = parse_email(_message("café".encode(), "base64", charset=label))

    assert [t.strip() for t in mail.text_plain] == ["café"]


def test__a_non_utf8_charset_still_decodes_through_encoding_rs():
    body = "café".encode("iso-8859-1")
    mail = parse_email(_message(body, "base64", charset="iso-8859-1"))

    assert [t.strip() for t in mail.text_plain] == ["café"]


def test__an_unknown_charset_falls_back_to_ascii_and_warns():
    mail = parse_email(_message("café".encode(), "base64", charset="not-a-charset"))

    assert mail.warnings, "an unrecognised label is a lossy repair and must warn"
    assert "�" in mail.text_plain[0]
