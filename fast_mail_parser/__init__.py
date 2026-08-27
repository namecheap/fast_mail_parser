from collections.abc import Iterator
from typing import TypeVar

from .fast_mail_parser import (
    DecodeError,
    HeaderParseError,
    MimeStructureError,
    ParseError,
    ParseWarning,
    PyAddress,
    PyAttachment,
    PyAttachmentMetadata,
    PyLazyAttachment,
    PyLazyMail,
    PyLazyMimePart,
    PyMail,
    PyMailMetadata,
    PyMimePart,
    PyMimePartMetadata,
    parse_email,
    parse_email_tree,
    parse_many,
)

# `walk` reads only `children`, which every tree node type has, so one
# implementation serves all three modes. The TypeVar is what makes that visible
# to a type checker: walking a `PyLazyMimePart` yields `PyLazyMimePart`, not the
# `PyMimePart` the single-mode signature used to promise (#202).
_Node = TypeVar("_Node", PyMimePart, PyLazyMimePart, PyMimePartMetadata)


def walk(part: _Node) -> Iterator[_Node]:
    """Yield `part` and every part beneath it, depth first.

    The same order as the stdlib's `email.message.Message.walk`, so code written
    against that reads the same here.

    Accepts a node from any `parse_email_tree` mode: the three node types differ
    in what a leaf's bytes cost and not in the shape of the tree.

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
    "ParseError",
    "HeaderParseError",
    "MimeStructureError",
    "DecodeError",
]
