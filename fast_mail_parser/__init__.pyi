__all__ = [
    "parse_email",
    "PyMail",
    "PyAttachment",
    "ParseError",
]

class PyAttachment:
    def __init__(self, mimetype: str, content: bytes, filename: str) -> None:
        self.mimetype = mimetype
        self.content = content
        self.filename = filename


class PyMail:
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
    """

