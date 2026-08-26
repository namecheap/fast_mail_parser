# Migrating to fast_mail_parser

Two audiences: people **upgrading from 0.6.x**, where two breaking changes
landed in 0.7.0, and people **coming from the stdlib `email` module**.

Every Python block below is executed by `tests/test_docs_snippets.py` against the
built wheel, in document order and in one shared namespace. A snippet that stops
working fails CI, so nothing here can rot.

## Setup

```python
from fast_mail_parser import ParseError, parse_email

raw = (
    b"From: Jane Doe <jane@example.com>\r\n"
    b"To: ops@example.com, \"Doe, John\" <john@example.com>\r\n"
    b"Subject: Quarterly report\r\n"
    b"Date: Mon, 01 Jan 2024 12:00:00 +0000\r\n"
    b"Received: from mx1.example.com by mx2.example.com\r\n"
    b"Received: from relay.example.com by mx1.example.com\r\n"
    b"MIME-Version: 1.0\r\n"
    b'Content-Type: multipart/mixed; boundary="b1"\r\n'
    b"\r\n"
    b"--b1\r\n"
    b"Content-Type: text/plain; charset=utf-8\r\n"
    b"\r\n"
    b"The report is attached.\r\n"
    b"--b1\r\n"
    b"Content-Type: text/plain; charset=utf-8\r\n"
    b'Content-Disposition: attachment; filename="notes.txt"\r\n'
    b"\r\n"
    b"attached notes\r\n"
    b"--b1--\r\n"
)

mail = parse_email(raw)
```

`parse_email` accepts `bytes` or `str`. Prefer `bytes`: reading a `.eml` in text
mode can mangle a message whose body is raw UTF-8 under 8BITMIME.

## Upgrading from 0.6.x

### `headers` is now `dict[str, list[str]]`

Previously a repeated header kept only its **last** value, so every earlier
`Received`, `DKIM-Signature` and `Received-SPF` was silently discarded. Each key
now maps to every value it appeared with, in order.

```python
# 0.7.0 — all hops, in message order
assert mail.headers["Received"] == [
    "from mx1.example.com by mx2.example.com",
    "from relay.example.com by mx1.example.com",
]

# Single-valued headers are one-element lists, so you never branch on the type.
assert mail.headers["Subject"] == ["Quarterly report"]
```

To migrate, index the list:

```python
# before:  subject = mail.headers["Subject"]
# after:
subject = mail.headers["Subject"][0]

# ...or, tolerating a missing header:
sender = mail.headers.get("From", [""])[0]
```

`subject` and `date` are unaffected — they are read from the parsed headers
directly, not out of this map.

### `attachments` no longer contains body parts or MIME containers

Previously every node of the MIME tree was reported, so a one-image message
returned four "attachments": the image, both text bodies, and the
`multipart/mixed` container. Bodies and attachments are now disjoint.

```python
# One real attachment, not four nodes.
assert len(mail.attachments) == 1
assert mail.attachments[0].filename == "notes.txt"

# The body is in text_plain, and the attachment's text is NOT mixed into it.
assert "The report is attached." in mail.text_plain[0]
assert not any("attached notes" in part for part in mail.text_plain)
```

Classification follows RFC 2183: a part is body text when it is `text/plain` or
`text/html` **and** is not marked `Content-Disposition: attachment`. If you
previously filtered containers out by hand, delete that code.

```python
# before: [a for a in mail.attachments if a.filename] or similar hand-filtering
# after:
attachments = mail.attachments
```

## Coming from the stdlib `email` module

| Task | stdlib `email` | fast_mail_parser |
| --- | --- | --- |
| Parse | `email.message_from_bytes(raw, policy=policy.default)` | `parse_email(raw)` |
| Subject | `msg["Subject"]` | `mail.subject` |
| One header | `msg["X"]` | `mail.headers["X"][0]` |
| All values of a header | `msg.get_all("Received")` | `mail.headers["Received"]` |
| Sender | `msg["From"]` (a string) | `mail.from_` (parsed) |
| Recipients | `msg["To"]` (a string) | `mail.to` (parsed list) |
| Plain body | `msg.get_body(("plain",)).get_content()` | `mail.text_plain[0]` |
| Attachments | `msg.iter_attachments()` | `mail.attachments` |
| Failure mode | varies; often silent | raises `ParseError` |

### Addresses are parsed, not handed back as strings

The stdlib gives you the raw header and leaves RFC 5322 syntax to you. Note the
second recipient below: a quoted display name containing a comma, which naive
splitting on `,` gets wrong.

```python
assert mail.from_ is not None
assert mail.from_.display_name == "Jane Doe"
assert mail.from_.address == "jane@example.com"

assert [(a.display_name, a.address) for a in mail.to] == [
    (None, "ops@example.com"),
    ("Doe, John", "john@example.com"),
]
```

RFC 5322 groups (`To: team: a@x, b@x;`) are flattened to their members. A header
that does not parse yields an empty list — or `None` for `from_` — rather than
raising, and the raw value stays in `headers`.

### Resolving inline images

```python
import re

by_cid = {a.content_id: a for a in mail.attachments if a.content_id}
for html in mail.text_html:
    for cid in re.findall(r'cid:([^"\'>\s]+)', html):
        attachment = by_cid.get(cid)
```

`content_id` is exposed without angle brackets, which is the form RFC 2392
`cid:` URLs use.

### Dates

`date` is the raw header string, as before. `date_parsed` resolves it to a
timezone-aware `datetime` in UTC, computed on access:

```python
from datetime import timezone

assert mail.date == "Mon, 01 Jan 2024 12:00:00 +0000"
assert mail.date_parsed is not None
assert mail.date_parsed.tzinfo == timezone.utc
assert mail.date_parsed.timestamp() == 1704110400
```

Unlike the stdlib's `email.utils.parsedate_to_datetime`, which raises on
malformed input, an unparseable header yields `None` and leaves `date` intact:

```python
undated = parse_email(b"Subject: x\r\nDate: not a date\r\n\r\nbody\r\n")
assert undated.date_parsed is None
assert undated.date == "not a date"
```

The instant is exact; the header's original UTC offset is not retained, so read
`date` if you need the sender's local offset.

### Errors

```python
handled = False
try:
    parse_email(b" unexpected continuation\r\n\r\nbody")
except ParseError:
    handled = True
assert handled
```

A malformed transfer encoding also raises `ParseError` rather than silently
yielding an empty body.

## What this library does not do

The stdlib is a full email *library*; this is a fast **parser**. Not provided:

- **Building or mutating messages.** There is no `set_content`, no serialization.
  Use `email.message.EmailMessage` to construct mail.
- **A `Message`-compatible object.** The API is deliberately its own shape; there
  is no drop-in adapter.
- **Header mutation.** `headers` is a plain dict snapshot; changing it changes
  nothing.
- **Python ≤ 3.10 or PyPy.** CPython 3.11+ only.
