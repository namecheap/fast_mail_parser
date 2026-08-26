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
    """

    def __init__(self, mimetype: str, content: bytes, filename: str) -> None:
        self.mimetype = mimetype
        self.content = content
        self.filename = filename


class PyMail:
    """A parsed message.

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
        headers: dict[str, str],
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

