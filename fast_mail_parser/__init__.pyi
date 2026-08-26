__all__ = [
    "parse_email",
    "PyMail",
    "PyAttachment",
    "ParseError",
]

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
        attachments: list[PyAttachment],
        headers: dict[str, list[str]],
    ) -> None:
        self.subject = subject
        self.text_plain = text_plain
        self.text_html = text_html
        self.date = date
        self.attachments = attachments
        self.headers = headers


class ParseError(Exception):
    """Error happened during parsing email."""


def parse_email(payload: str | bytes) -> PyMail:
    """Parse raw content of email and return structured datatype.

    A missing ``Subject`` or ``Date`` header yields the empty string ``""``
    (not ``None``) on the returned ``PyMail``.

    Raises ``ParseError`` if the payload cannot be parsed, including when a
    part's ``Content-Transfer-Encoding`` is malformed.
    """

