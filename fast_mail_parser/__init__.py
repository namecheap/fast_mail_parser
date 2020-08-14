from typing import Union, List, Dict

__all__ = [
    'parse_email',
]


class PyAttachment:
    mimetype: str
    content: bytes


class PyMail:
    subject: str
    text_plain: List[str]
    text_html: List[str]
    date: str
    attachments: List[PyAttachment]
    headers: Dict[str, str]


class ParseError(Exception):
    pass


def parse_email(payload: Union[str, bytes]) -> PyMail:
    ...


from .fast_mail_parser import parse_email, ParseError
