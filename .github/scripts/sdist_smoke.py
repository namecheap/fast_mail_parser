#!/usr/bin/env python3
"""Smoke-test a source install of fast_mail_parser.

Run against a venv that installed the sdist, so the extension was compiled here
rather than downloaded. The point is not coverage -- the full suite runs against
the wheel -- but that every entry point the package advertises exists and works
in a build nobody else exercises.
"""
import sys

import fast_mail_parser
from fast_mail_parser import (
    ParseError,
    parse_email,
    parse_email_tree,
    parse_many,
    walk,
)

MESSAGE = (
    b"From: sender@example.com\r\n"
    b"To: recipient@example.com\r\n"
    b"Subject: sdist smoke\r\n"
    b'Content-Type: multipart/alternative; boundary="bnd"\r\n'
    b"\r\n"
    b"--bnd\r\n"
    b"Content-Type: text/plain\r\n"
    b"\r\n"
    b"plain body\r\n"
    b"--bnd\r\n"
    b"Content-Type: text/html\r\n"
    b"\r\n"
    b"<p>html body</p>\r\n"
    b"--bnd--\r\n"
)


def check(label: str, condition: bool) -> None:
    print(f"{'ok  ' if condition else 'FAIL'} {label}")
    if not condition:
        sys.exit(f"source install failed: {label}")


mail = parse_email(MESSAGE)
check("parse_email subject", mail.subject == "sdist smoke")
check("parse_email bodies", mail.text_plain == ["plain body"])
check("headers keep wire order", list(mail.headers)[0] == "From")
check("addresses parsed", mail.from_.address == "sender@example.com")
check("warnings channel present and empty", mail.warnings == [])

meta = parse_email(MESSAGE, mode="metadata")
check("metadata mode agrees on subject", meta.subject == mail.subject)
check("metadata mode has no bodies", not hasattr(meta, "text_plain"))

root = parse_email_tree(MESSAGE)
types = [part.content_type for part in walk(root)]
check(
    "tree topology",
    types == ["multipart/alternative", "text/plain", "text/html"],
)

batch = parse_many([MESSAGE, MESSAGE])
check("parse_many returns both", len(batch) == 2)

try:
    parse_email(b" unexpected continuation\r\n\r\nbody")
except ParseError:
    check("errors raise ParseError", True)
else:
    check("errors raise ParseError", False)

check("py.typed shipped", (
    __import__("pathlib").Path(fast_mail_parser.__file__).parent / "py.typed"
).exists())

print(f"\nsource install verified: {len(fast_mail_parser.__all__)} exports")
