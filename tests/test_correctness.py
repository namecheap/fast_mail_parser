"""Round-trip / property-style correctness tests.

These build well-formed messages with Python's stdlib ``email`` library
(``EmailMessage`` + ``BytesGenerator``), serialize them to bytes, parse the
bytes back with ``fast_mail_parser.parse_email``, and assert the parsed fields
match what was put in. The stdlib is the trusted producer here, so any
divergence points at the parser under test.

Scope and rationale:
- Stdlib only — no ``hypothesis`` (kept out of the dependency set on purpose).
  We get property/round-trip coverage by enumerating several concrete message
  *shapes* (plain, multipart/alternative, with attachment) instead.
- We assert on values we control end-to-end: subject, plain/HTML bodies,
  attachment count / mimetype / raw content bytes, and ordinary single-valued
  header values.

Characterized behavior (current parser contract, intentionally pinned):
- ``attachments`` holds only real attachments. ``multipart/*`` container nodes
  are MIME structure and are not reported, and body parts belong to
  ``text_plain`` / ``text_html``. So a multipart/alternative message has an
  empty attachment list.
- RFC 2183 decides body-vs-attachment: a part is body text when it is
  ``text/plain`` or ``text/html`` and is not marked ``Content-Disposition:
  attachment``. ``EmailMessage.add_attachment`` always marks parts
  ``attachment``, so even its text/plain parts are attachments, not body.
- ``filename`` prefers the Content-Disposition ``filename`` parameter and falls
  back to Content-Type ``name``. ``add_attachment`` emits only the former, and
  it surfaces correctly.
- Stdlib bodies are emitted with a trailing newline; the parser preserves it,
  so body comparisons use membership / ``strip`` rather than strict equality.
"""

from email.generator import BytesGenerator
from email.message import EmailMessage
from io import BytesIO

import pytest

from fast_mail_parser import PyAttachment, PyMail, parse_email


def _serialize(message: EmailMessage) -> bytes:
    """Render an EmailMessage to wire-format bytes with CRLF line endings."""
    buffer = BytesIO()
    BytesGenerator(buffer, policy=message.policy.clone(linesep="\r\n")).flatten(message)
    return buffer.getvalue()


def _attachment_by_mimetype(mail: PyMail, mimetype: str) -> PyAttachment:
    for attachment in mail.attachments:
        if attachment.mimetype == mimetype:
            return attachment
    raise AssertionError(f"no attachment with mimetype {mimetype!r}")


# --- plain text -------------------------------------------------------------


def test__plain_text_round_trip():
    message = EmailMessage()
    message["Subject"] = "Plain round trip"
    message["From"] = "alice@example.com"
    message["To"] = "bob@example.com"
    message.set_content("Hello, this is the plain body.\n")

    mail = parse_email(_serialize(message))

    assert mail.subject == "Plain round trip"
    assert mail.headers["From"] == ["alice@example.com"]
    assert mail.headers["To"] == ["bob@example.com"]
    assert len(mail.text_plain) == 1
    assert "Hello, this is the plain body." in mail.text_plain[0]
    assert mail.text_html == []


# --- multipart/alternative --------------------------------------------------


def test__multipart_alternative_round_trip():
    plain = "Plain alternative body."
    html = "<html><body><p>HTML alternative body.</p></body></html>"

    message = EmailMessage()
    message["Subject"] = "Alternative round trip"
    message["From"] = "alice@example.com"
    message.set_content(plain + "\n")
    message.add_alternative(html + "\n", subtype="html")

    mail = parse_email(_serialize(message))

    assert mail.subject == "Alternative round trip"

    assert len(mail.text_plain) == 1
    assert plain in mail.text_plain[0]

    assert len(mail.text_html) == 1
    assert html in mail.text_html[0]

    # Both text parts are bodies and the container is MIME structure, so nothing
    # is reported as an attachment.
    assert mail.attachments == []


# --- multipart/mixed with a binary attachment -------------------------------


def test__attachment_round_trip_preserves_content_and_mimetype():
    payload = bytes(range(256))  # every byte value, to prove binary-safety

    message = EmailMessage()
    message["Subject"] = "Attachment round trip"
    message["From"] = "alice@example.com"
    message.set_content("See attached.\n")
    message.add_attachment(
        payload,
        maintype="application",
        subtype="octet-stream",
        filename="payload.bin",
    )

    mail = parse_email(_serialize(message))

    assert mail.subject == "Attachment round trip"
    assert "See attached." in mail.text_plain[0]

    attachment = _attachment_by_mimetype(mail, "application/octet-stream")
    assert attachment.content == payload  # exact bytes survive base64 round trip
    # add_attachment emits the filename via Content-Disposition only, which the
    # parser now reads (RFC 2183).
    assert attachment.filename == "payload.bin"


def test__text_attachment_content_round_trips():
    body = "line one\nline two\n"

    message = EmailMessage()
    message["Subject"] = "Text attachment"
    message["From"] = "alice@example.com"
    message.set_content("Body before attachment.\n")
    message.add_attachment(
        body.encode("utf-8"),
        maintype="text",
        subtype="plain",
        filename="notes.txt",
    )

    mail = parse_email(_serialize(message))

    # `add_attachment` marks the part `Content-Disposition: attachment`, so this
    # text/plain part is a file rather than body text -- its lines must not leak
    # into text_plain -- and it is now locatable by its disposition filename.
    assert "Body before attachment." in mail.text_plain[0]
    assert not any("line one" in part for part in mail.text_plain)

    attachment = next(a for a in mail.attachments if a.filename == "notes.txt")
    assert attachment.mimetype == "text/plain"
    assert attachment.content.decode("utf-8") == body


# --- header value fidelity --------------------------------------------------


@pytest.mark.parametrize(
    "key, value",
    [
        ("X-Custom-Token", "abc-123-DEF"),
        ("X-Numeric", "00420"),
        ("Reply-To", "Support <support@example.com>"),
    ],
)
def test__single_valued_header_round_trips(key: str, value: str):
    message = EmailMessage()
    message["Subject"] = "Header fidelity"
    message["From"] = "alice@example.com"
    message[key] = value
    message.set_content("body\n")

    mail = parse_email(_serialize(message))

    assert mail.headers[key] == [value]


def test__returns_pymail_for_every_shape():
    # A light invariant guard: each builder above produces a parseable PyMail.
    message = EmailMessage()
    message["Subject"] = "Invariant"
    message["From"] = "alice@example.com"
    message.set_content("body\n")

    mail = parse_email(_serialize(message))

    assert isinstance(mail, PyMail)
    assert isinstance(mail.subject, str) and mail.subject
    assert isinstance(mail.headers, dict) and mail.headers
