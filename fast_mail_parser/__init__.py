from collections.abc import Iterator

from .fast_mail_parser import (
    DecodeError,
    HeaderParseError,
    MimeStructureError,
    ParseError,
    PyAddress,
    PyAttachment,
    PyMail,
    PyMimePart,
    parse_email,
    parse_email_tree,
    parse_many,
)


def walk(part: PyMimePart) -> Iterator[PyMimePart]:
    """Yield `part` and every part beneath it, depth first.

    The same order as the stdlib's `email.message.Message.walk`, so code written
    against that reads the same here.

    Written in Python rather than Rust deliberately: it is a generator, so a
    caller that stops early -- the common case, looking for the first part of some
    type -- does not pay for the rest of the walk.
    """
    yield part
    for child in part.children:
        yield from walk(child)


__all__ = [
    "parse_email",
    "parse_email_tree",
    "parse_many",
    "walk",
    "PyMail",
    "PyMimePart",
    "PyAttachment",
    "PyAddress",
    "ParseError",
    "HeaderParseError",
    "MimeStructureError",
    "DecodeError",
]
