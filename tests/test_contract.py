"""Public API contract tests.

fast_mail_parser has many downstream consumers, so its public surface must not
drift accidentally — for example, a native-binding upgrade silently renaming,
dropping, or retyping an attribute. These tests freeze that surface: the exact
exported names, the attribute set and types of PyMail / PyAttachment, the input
types parse_email accepts, and the errors it raises.

The sets below are frozen, not descriptive: adding to one is a deliberate act.
`warnings` on PyMail and the `ParseWarning` export were added that way for the
parse-warnings channel (#100) -- purely additive, so nothing an existing
consumer reads changed name or type. `PyLazyMail`/`PyLazyAttachment` (#97) were
added the same way, and are new types rather than changes to these ones for
exactly that reason: `PyAttachment.content` still costs what it always cost and
still raises where it always raised. `PyLazyMimePart`/`PyMimePartMetadata` (#202)
extend that pattern to the tree, for the same reason: `PyMimePart.content` is
still a value rather than a decode, and its `None` still means "container" and
nothing else.

A failure here means a consumer-visible change. Treat it as a deliberate API
break (update consumers, bump the version) — do not loosen the test to make it
pass.
"""


import pytest

import fast_mail_parser
from fast_mail_parser import ParseError, ParseWarning, PyAttachment, PyMail, parse_email

EXPECTED_EXPORTS = {
    "parse_email",
    "parse_email_tree",
    "parse_many",
    "walk",
    "ParseError",
    "HeaderParseError",
    "MimeStructureError",
    "DecodeError",
    "PyMail",
    "PyLazyMail",
    "PyMailMetadata",
    "PyMimePart",
    "PyLazyMimePart",
    "PyMimePartMetadata",
    "PyAttachment",
    "PyLazyAttachment",
    "PyAttachmentMetadata",
    "PyAddress",
    "ParseWarning",
}
EXPECTED_MAIL_ATTRS = {
    "subject",
    "text_plain",
    "text_html",
    "date",
    "date_parsed",
    "from_",
    "to",
    "cc",
    "bcc",
    "reply_to",
    "attachments",
    "headers",
    # Added deliberately with the parse-warnings channel (#100). Additive: no
    # existing attribute changed name or type, so an `except`/getattr written
    # against the old surface is unaffected.
    "warnings",
}
EXPECTED_ATTACHMENT_ATTRS = {
    "mimetype",
    "content",
    "filename",
    "content_id",
    "disposition",
}
EXPECTED_WARNING_ATTRS = {
    "kind",
    "part_path",
    "detail",
}

# An unrecognised charset label: mailparse's own test suite pins `x-unknown` as
# a label the charset crate does not resolve, so this is the documented way to
# reach the us-ascii fallback rather than a guess at what is unrecognised.
CHARSET_FALLBACK_MESSAGE = (
    b"Subject: fallback\r\n"
    b"Content-Type: text/plain; charset=x-unknown\r\n"
    b"\r\n"
    b"body\r\n"
)

# First line starts with whitespace, so it is an overhanging continuation with
# no preceding header key — mailparse rejects this, surfacing as ParseError.
MALFORMED_MESSAGE = " unexpected continuation\r\n\r\nbody"


def _public_attrs(obj: object) -> set[str]:
    return {name for name in dir(obj) if not name.startswith("_")}


# --- module exports ---------------------------------------------------------


def test__public_exports_are_frozen():
    assert set(fast_mail_parser.__all__) == EXPECTED_EXPORTS


def test__all_exports_are_importable():
    for name in EXPECTED_EXPORTS:
        assert hasattr(fast_mail_parser, name), f"missing export: {name}"


# --- PyMail surface ---------------------------------------------------------


def test__pymail_attribute_set_is_frozen(valid_mail: PyMail):
    assert _public_attrs(valid_mail) == EXPECTED_MAIL_ATTRS


def test__pymail_attribute_types(valid_mail: PyMail):
    assert isinstance(valid_mail.subject, str)
    assert isinstance(valid_mail.date, str)

    assert isinstance(valid_mail.text_plain, list)
    assert all(isinstance(item, str) for item in valid_mail.text_plain)

    assert isinstance(valid_mail.text_html, list)
    assert all(isinstance(item, str) for item in valid_mail.text_html)

    assert isinstance(valid_mail.headers, dict)
    assert all(
        isinstance(key, str)
        and isinstance(values, list)
        and all(isinstance(value, str) for value in values)
        for key, values in valid_mail.headers.items()
    )

    assert isinstance(valid_mail.attachments, list)

    assert isinstance(valid_mail.warnings, list)


# --- PyAttachment surface ---------------------------------------------------


def test__pyattachment_attribute_set_is_frozen(attachment_mail: PyMail):
    assert attachment_mail.attachments, "fixture must contain attachments"
    for attachment in attachment_mail.attachments:
        assert _public_attrs(attachment) == EXPECTED_ATTACHMENT_ATTRS


def test__pyattachment_attribute_types(attachment_mail: PyMail):
    assert attachment_mail.attachments, "fixture must contain attachments"
    for attachment in attachment_mail.attachments:
        assert isinstance(attachment, PyAttachment)
        assert isinstance(attachment.mimetype, str)
        assert isinstance(attachment.filename, str)
        # content must stay `bytes` (not e.g. list[int]) for binary-safe consumers.
        assert isinstance(attachment.content, bytes)


# --- ParseWarning surface ---------------------------------------------------


def test__parsewarning_attribute_set_is_frozen():
    mail = parse_email(CHARSET_FALLBACK_MESSAGE)

    assert mail.warnings, "fixture must produce a warning"
    for warning in mail.warnings:
        assert _public_attrs(warning) == EXPECTED_WARNING_ATTRS


def test__parsewarning_attribute_types():
    mail = parse_email(CHARSET_FALLBACK_MESSAGE)

    assert mail.warnings, "fixture must produce a warning"
    for warning in mail.warnings:
        assert isinstance(warning, ParseWarning)
        # All three are `str`, including part_path -- "" rather than None for a
        # message-level warning, so consumers never branch on the type.
        assert isinstance(warning.kind, str)
        assert isinstance(warning.part_path, str)
        assert isinstance(warning.detail, str)


# --- parse_email input contract ---------------------------------------------


def test__returns_pymail(valid_message: str):
    assert isinstance(parse_email(valid_message), PyMail)


def test__accepts_str_and_bytes_equivalently(valid_message: str):
    from_str = parse_email(valid_message)
    from_bytes = parse_email(valid_message.encode("utf-8"))

    assert from_str.subject == from_bytes.subject
    assert from_str.date == from_bytes.date
    assert from_str.headers == from_bytes.headers
    assert from_str.text_plain == from_bytes.text_plain
    assert from_str.text_html == from_bytes.text_html


@pytest.mark.parametrize("bad_input", [123, None, 4.5, ["not", "bytes"], {"a": 1}])
def test__rejects_non_text_input_with_type_error(bad_input):
    with pytest.raises(TypeError):
        parse_email(bad_input)


def test__non_ascii_str_decodes_as_utf8():
    # str input is decoded losslessly as its UTF-8 bytes. A non-ASCII str must
    # therefore parse exactly like its explicit UTF-8 encoding (no truncation of
    # code points to their low byte).
    raw = "Subject: café über 日本\r\n\r\nbody"

    from_str = parse_email(raw)
    from_bytes = parse_email(raw.encode("utf-8"))

    assert from_str.subject == from_bytes.subject
    assert from_str.headers == from_bytes.headers


# --- error contract ---------------------------------------------------------


def test__parse_error_is_exception_subclass():
    assert issubclass(ParseError, Exception)


def test__malformed_message_raises_parse_error():
    with pytest.raises(ParseError):
        parse_email(MALFORMED_MESSAGE)
