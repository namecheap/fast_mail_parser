from datetime import datetime

__all__ = [
    "parse_email",
    "PyMail",
    "PyAttachment",
    "PyAddress",
    "ParseError",
]


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
    ``headers["From"] == ["a@example.com"]``.

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
    """Error happened during parsing email."""


def parse_email(payload: str | bytes) -> PyMail:
    """Parse raw content of email and return structured datatype.

    A missing ``Subject`` or ``Date`` header yields the empty string ``""``
    (not ``None``) on the returned ``PyMail``.

    Raises ``ParseError`` if the payload cannot be parsed, including when a
    part's ``Content-Transfer-Encoding`` is malformed.
    """

