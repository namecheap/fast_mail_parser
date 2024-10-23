import typing as t

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
        text_plain: t.List[str],
        text_html: t.List[str],
        date: str,
        attachments: t.List[PyAttachment],
        headers: t.Dict[str, str],
    ) -> None:
        self.subject = subject
        self.text_plain = text_plain
        self.text_html = text_html
        self.date = date
        self.attachments = attachments
        self.headers = headers


class ParseError(Exception):
    """Error happened during parsing email."""


def parse_email(payload: t.Union[str, bytes]) -> PyMail:
    """Parse raw content of email and return structured datatype."""

