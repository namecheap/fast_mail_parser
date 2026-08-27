from collections.abc import Iterator
from datetime import datetime

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


class PyMimePart:
    """One node of a message's MIME tree, with the structure intact.

    ``PyMail`` is a flattened projection of this -- bodies in one list,
    attachments in another, containers dropped. Every flattening loses something:
    which ``text/html`` part corresponds to which ``text/plain`` sibling, whether
    a node was ``multipart/alternative`` or ``multipart/mixed``, where a bounce's
    inner message begins.

    ``content`` is the transfer-decoded bytes of a leaf, and ``None`` for a
    ``multipart/*`` container -- whose body is just its children with boundaries
    between them.

    ``is_message`` is ``True`` for ``message/rfc822``: the embedded message's own
    root is this part's single child, so a bounce's headers are reachable rather
    than opaque. Such nesting counts against the same recursion cap as a
    multipart tree.

    ``headers`` keeps every value of every header, keys in the order the names
    first appeared -- the same semantics as ``PyMail.headers``.
    """

    content_type: str
    headers: dict[str, list[str]]
    filename: str
    content_id: str | None
    disposition: str | None
    is_message: bool
    content: bytes | None
    children: list[PyMimePart]


class PyAddress:
    """One mailbox from an address header.

    ``display_name`` is ``None`` when the header carries a bare address. RFC 2047
    encoded-words are decoded, so a non-ASCII name arrives readable.
    ``address`` is the ``addr-spec`` -- ``local@domain``, without angle brackets.
    """

    def __init__(self, display_name: str | None, address: str) -> None:
        self.display_name = display_name
        self.address = address

class PyAttachment:
    """A non-body MIME part: a real attachment or an inline resource.

    ``filename`` is taken from the ``Content-Disposition`` ``filename``
    parameter (RFC 2183, including RFC 2231 extended values), falling back to
    the ``Content-Type`` ``name`` parameter. It is ``""`` when the part declares
    neither -- common for inline images referenced only by ``Content-ID``.

    ``content_id`` is the part's ``Content-ID`` with angle brackets stripped, or
    ``None``. RFC 2392 ``cid:`` URLs in an HTML body reference this bracket-less
    form, so resolving inline images is a lookup keyed on this value.

    ``disposition`` is the raw ``Content-Disposition`` token (typically
    ``"inline"`` or ``"attachment"``), or ``None`` when the part declares no such
    header -- an absent header is reported distinctly from an explicit
    ``inline``.
    """

    def __init__(
        self,
        mimetype: str,
        content: bytes,
        filename: str,
        content_id: str | None,
        disposition: str | None,
    ) -> None:
        self.mimetype = mimetype
        self.content = content
        self.filename = filename
        self.content_id = content_id
        self.disposition = disposition


class PyMail:
    """A parsed message.

    ``date_parsed`` is ``date`` resolved to a timezone-aware ``datetime`` in UTC,
    or ``None`` when the header is absent or unparseable. It is computed on
    access, so reading it is the only cost. The instant is exact; the header's
    original offset is not retained -- read ``date`` for that.

    Address headers are parsed into ``PyAddress`` values, taken from the first
    occurrence of each header. RFC 5322 groups (``To: team: a@x, b@x;``) are
    flattened to their members. An address header that does not parse yields an
    empty list (or ``None`` for ``from_``) rather than raising -- a malformed
    ``To:`` must not fail an otherwise good message, and the raw value stays
    available through ``headers``.

    ``headers`` maps each header name to **every** value it appeared with, in
    order, so repeated keys such as ``Received`` or ``DKIM-Signature`` are
    preserved. Single-valued headers are one-element lists:
    ``headers["From"] == ["a@example.com"]``. The keys are in the order the names
    first appeared in the message, stably across parses.

    Body parts and attachments are disjoint. A part is body text -- reaching
    ``text_plain`` or ``text_html`` -- when it is ``text/plain`` or ``text/html``
    and is not marked ``Content-Disposition: attachment``; every other part is an
    attachment. ``multipart/*`` container nodes are MIME structure and appear in
    neither list.
    """

    def __init__(
        self,
        subject: str,
        text_plain: list[str],
        text_html: list[str],
        date: str,
        from_: PyAddress | None,
        to: list[PyAddress],
        cc: list[PyAddress],
        bcc: list[PyAddress],
        reply_to: list[PyAddress],
        attachments: list[PyAttachment],
        headers: dict[str, list[str]],
    ) -> None:
        self.subject = subject
        self.text_plain = text_plain
        self.text_html = text_html
        self.date = date
        self.from_ = from_
        self.to = to
        self.cc = cc
        self.bcc = bcc
        self.reply_to = reply_to
        self.attachments = attachments
        self.headers = headers

    @property
    def date_parsed(self) -> datetime | None: ...


class ParseError(Exception):
    """Base class for every parse failure.

    ``except ParseError`` catches all of the subtypes below, so code written
    against it keeps working.
    """


class HeaderParseError(ParseError):
    """The header section could not be parsed.

    Usually means the input is not an email at all.
    """


class MimeStructureError(ParseError):
    """The MIME structure is malformed, or a resource cap was exceeded.

    Raised for a payload over the 100 MiB input limit and for MIME nesting
    deeper than 256 levels -- both hostile-input guards rather than ordinary
    malformedness.
    """


class DecodeError(ParseError):
    """A part's ``Content-Transfer-Encoding`` could not be decoded.

    For example base64 or quoted-printable that does not decode. The rest of the
    message may still have been well-formed.
    """


def parse_email(payload: str | bytes) -> PyMail:
    """Parse raw content of email and return structured datatype.

    A missing ``Subject`` or ``Date`` header yields the empty string ``""``
    (not ``None``) on the returned ``PyMail``.

    Raises a ``ParseError`` subtype: ``HeaderParseError``,
    ``MimeStructureError`` or ``DecodeError``. Catch ``ParseError`` to handle
    all of them.
    """


def parse_email_tree(payload: str | bytes) -> PyMimePart:
    """Parse a message into its MIME tree, structure intact.

    Additive: ``parse_email`` is unchanged. Accepts the same payloads and raises
    the same ``ParseError`` subtypes, including the size and recursion caps.

    Use this when the shape of the message matters -- forensics, bounce
    processing, deciding which body belongs to which alternative -- and
    ``parse_email`` when the flat projection is what you want.
    """


def walk(part: PyMimePart) -> Iterator[PyMimePart]:
    """Yield ``part`` and every part beneath it, depth first.

    The same order as the stdlib's ``email.message.Message.walk``. It is a
    generator, so stopping early costs nothing for the rest of the tree.
    """
def parse_many(
    payloads: list[str | bytes],
    *,
    threads: int | None = None,
    raise_on_error: bool = False,
) -> list[PyMail | ParseError]:
    """Parse a batch of messages in one call, in parallel, in input order.

    Each slot of the result is either a ``PyMail`` or a ``ParseError``
    *instance* -- returned, not raised -- so one malformed message does not cost
    the caller the rest of the batch, and inputs zip cleanly to outcomes::

        for payload, outcome in zip(payloads, parse_many(payloads)):
            if isinstance(outcome, ParseError):
                ...

    Pass ``raise_on_error=True`` to raise the first failure instead.

    ``threads`` caps the worker count; the default is the machine's parallelism.
    ``threads=0`` raises ``ValueError`` -- pass ``None`` for the default. The GIL
    is released for the whole batch rather than per message.

    Every parsed message is materialised before returning, so chunk large
    workloads at the caller.

    Best suited to **many small messages**: the overhead it removes scales with
    the message count, while its one cost -- copying each payload before parsing
    begins -- scales with total bytes. For a few very large messages a Python
    thread pool over ``parse_email`` is currently faster. See the README for
    measured figures.
    """
