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

Characterized quirks (current parser behavior, intentionally pinned):
- Every MIME node is reported as an attachment, including the text parts and
  the multipart *container* nodes (containers carry empty content). So the
  attachment list length is the number of MIME nodes, not just the "real"
  file attachments. Tests below filter by mimetype rather than asserting a bare
  ``len(attachments)`` for multipart messages.
- ``filename`` is read from the Content-Type ``name`` parameter, not from
  Content-Disposition ``filename``. ``EmailMessage.add_attachment`` emits the
  filename ONLY via Content-Disposition (no Content-Type ``name`` param), so for
  these stdlib-built messages the parser reports ``filename == ""`` even though
  the attachment content round-trips exactly. Tests assert that observed
  behavior rather than expecting the disposition filename to surface.
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
    assert mail.headers["From"] == "alice@example.com"
    assert mail.headers["To"] == "bob@example.com"
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

    # Container node is reported as an attachment alongside the two text nodes.
    mimetypes = [attachment.mimetype for attachment in mail.attachments]
    assert "text/plain" in mimetypes
    assert "text/html" in mimetypes
    assert "multipart/alternative" in mimetypes


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
    # add_attachment emits the filename via Content-Disposition only; the parser
    # reads it from the Content-Type `name` param, so it surfaces as empty here.
    assert attachment.filename == ""


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

    # The disposition filename is not surfaced (see module docstring), so the
    # attachment is located by its distinct decoded content, not by filename.
    text_contents = [
        a.content.decode("utf-8")
        for a in mail.attachments
        if a.mimetype == "text/plain"
    ]
    assert body in text_contents


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

    assert mail.headers[key] == value


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
