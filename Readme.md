# fast_mail_parser

![Test](https://github.com/namecheap/fast_mail_parser/workflows/Test/badge.svg)
[![PyPI version](https://badge.fury.io/py/fast-mail-parser-ng.svg)](https://badge.fury.io/py/fast-mail-parser-ng)
[![Downloads](https://pepy.tech/badge/fast-mail-parser-ng)](https://pepy.tech/project/fast-mail-parser-ng)

> ## 📦 Now published as `fast-mail-parser-ng`
>
> **Install it under the new name:**
>
> ```bash
> pip install fast-mail-parser-ng
> ```
>
> **Your code does not change.** The import path is still `fast_mail_parser`:
>
> ```python
> from fast_mail_parser import parse_email
> ```
>
> Migrating from `fast-mail-parser`? Change the name in your requirements file
> and nothing else — no code edits, same API.
>
> ```diff
> - fast-mail-parser
> + fast-mail-parser-ng
> ```
>
> Looking for the old `fast-mail-parser` package? It is a different,
> unmaintained upload frozen at 0.2.5 (June 2022) that this project cannot
> publish to. See [Why the name changed](#why-the-name-changed).

A very fast Python library for parsing `.eml` files. It is built on the Rust
[mailparse](https://github.com/staktrace/mailparse) crate via
[pyo3](https://github.com/PyO3/pyo3), and parses roughly **7–8x faster** than
pure-Python implementations.

## Quickstart

```bash
pip install fast-mail-parser-ng
```

```python
from fast_mail_parser import parse_email

with open("message.eml", "rb") as f:
    email = parse_email(f.read())

print(email.subject)
print(email.text_plain[0])
```

That is the whole surface for the common case. See [Usage](#usage) for the full
API, and [Python support](#python-support) for wheel coverage.

Coming from the stdlib `email` module, or upgrading from 0.6.x? See the
[migration guide](https://github.com/namecheap/fast_mail_parser/blob/master/docs/migrating.md) — its snippets are
executed in CI, so they cannot go stale — and
[compatibility.md](https://github.com/namecheap/fast_mail_parser/blob/master/docs/compatibility.md) for every known
difference from the stdlib, each one enforced by a test.

## Why the name changed

The `fast-mail-parser` name on PyPI belongs to a PyPI account this project no
longer controls, and it is frozen at an unmaintained **0.2.5 from June 2022**.
Only a project owner can publish to a name, so fixes could not reach it — the
PEP 541 transfer request
([pypi/support#11044](https://github.com/pypi/support/issues/11044)) has been
open and unattended since June 2026.

Rather than hold releases behind that queue indefinitely, this project publishes
under a name it owns. The import path was deliberately left as
`fast_mail_parser` so the change costs you one line in a requirements file and
no code. If the transfer is ever granted, `fast-mail-parser` will resume as an
alias.

Full history in the [changelog](https://github.com/namecheap/fast_mail_parser/blob/master/CHANGELOG.md).

## Python support

Wheels target the CPython stable ABI (`cp311-abi3`): one wheel per platform
covers every supported CPython version, including versions released after the
package — a new Python no longer has to wait for a new release.

| Python | Support |
| --- | --- |
| CPython 3.11+ (including future versions) | Prebuilt wheel |
| CPython 3.13t/3.14t (free-threaded) | Builds from source; the extension currently re-enables the GIL on import ([#101](https://github.com/namecheap/fast_mail_parser/issues/101)) |
| CPython ≤ 3.10 | Not supported (last compatible release: 0.2.5) |
| PyPy | Not supported |

13 prebuilt wheels ship per release: manylinux and musllinux across x86_64,
i686, aarch64, armv7, s390x and ppc64le; Windows x64 and x86; macOS arm64. Every
release is published via PyPI Trusted Publishing with PEP 740 attestations.

## Benchmark

Parsing the same message, `fast_mail_parser` against the pure-Python
`mail-parser`. CI enforces a floor of 7x on every pull request, so this margin
is a gate rather than a claim.

```
 -------------------------------------------------------------------------------------------- benchmark: 2 tests -------------------------------------------------------------------------------------------
Name (time in ms)                              Min                Max               Mean            StdDev             Median               IQR            Outliers       OPS            Rounds  Iterations
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
test__fast_mail_parser___parse_message      1.8136 (1.0)       1.8938 (1.0)       1.8426 (1.0)      0.0176 (1.0)       1.8465 (1.0)      0.0277 (1.0)         180;0  542.7141 (1.0)         450           1
test__mail_parser___parse_message          14.5583 (8.03)     15.8571 (8.37)     15.0264 (8.16)     0.2368 (13.49)    14.9702 (8.11)     0.2887 (10.42)         5;1   66.5495 (0.12)         32           1
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------

Legend:
  Outliers: 1 Standard Deviation from Mean; 1.5 IQR (InterQuartile Range) from 1st Quartile and 3rd Quartile.
  OPS: Operations Per Second, computed as 1 / Mean
```

## Usage

`parse_email` accepts the raw message as `str` or `bytes` and returns a
`PyMail`. It raises `ParseError` if the payload cannot be parsed.

`PyMail` exposes the following attributes:

| Attribute | Type | Description |
| --- | --- | --- |
| `subject` | `str` | Subject header (empty string if missing). |
| `date` | `str` | Date header (empty string if missing). |
| `date_parsed` | `datetime \| None` | `date` as a tz-aware UTC datetime; computed on access. |
| `from_` | `PyAddress \| None` | The `From` mailbox. Named `from_`; `from` is a keyword. |
| `to` / `cc` / `bcc` / `reply_to` | `list[PyAddress]` | Recipients, groups flattened. |
| `text_plain` | `list[str]` | All `text/plain` bodies. |
| `text_html` | `list[str]` | All `text/html` bodies. |
| `headers` | `dict[str, list[str]]` | All values of every header, in order. |
| `attachments` | `list[PyAttachment]` | Non-body parts (see below). |

Each `PyAttachment` has:

| Attribute | Type | Description |
| --- | --- | --- |
| `mimetype` | `str` | The part's media type. |
| `filename` | `str` | See below; `""` when the part declares none. |
| `content` | `bytes` | Decoded bytes, transfer-encoding undone. |
| `content_id` | `str \| None` | `Content-ID` with angle brackets stripped. |
| `disposition` | `str \| None` | Raw `Content-Disposition` token, or `None` if absent. |

### Addresses

Address headers are parsed rather than handed back as strings — RFC 5322 address
syntax (display names, quoted strings containing commas, groups, comments) is
exactly what hand-rolled regexes get wrong:

```python
mail.from_.display_name   # 'Jane Doe'  (None for a bare address)
mail.from_.address        # 'jane@example.com'

[a.address for a in mail.to]   # ['a@example.com', 'b@example.com']
```

- RFC 5322 groups (`To: team: a@x, b@x;`) are **flattened** to their member
  mailboxes; the group name is structure and is not exposed.
- RFC 2047 encoded display names are decoded, including inside quoted names.
- A header that does not parse yields an empty list (or `None` for `from_`)
  rather than raising — a malformed `To:` never fails an otherwise good message,
  and the raw value stays in `headers`.

### Headers

`headers` maps each header name to a **list** of every value it appeared with,
in message order, so repeated fields survive:

```python
mail.headers["Received"]   # ['from mx1...', 'from mx2...', 'from mx3...']
mail.headers["From"]       # ['sender@example.com'] -- always a list
```

`subject` and `date` are read from the parsed headers directly rather than out
of this map, so they always reflect the first occurrence of their field.

### Resolving inline images (`cid:`)

`content_id` is exposed without angle brackets, which is the form RFC 2392
`cid:` URLs use — so resolving the images an HTML body references is a lookup:

```python
import re

mail = parse_email(raw)
by_cid = {a.content_id: a for a in mail.attachments if a.content_id}

for cid in re.findall(r'cid:([^"\'>\s]+)', mail.text_html[0]):
    attachment = by_cid.get(cid)
    if attachment:
        print(cid, attachment.mimetype, len(attachment.content), "bytes")
```

`disposition` reports the part's raw `Content-Disposition` token, and
distinguishes an absent header (`None`) from an explicit `inline` — the two are
different statements about intent.

### Bodies vs. attachments

The two are disjoint — a part appears in exactly one place. Classification
follows RFC 2183 rather than the media type alone:

- A part is **body text** (`text_plain` / `text_html`) when it is `text/plain`
  or `text/html` **and** is not marked `Content-Disposition: attachment`. A
  `Content-Type; name` parameter does not change this — an inline text part
  stays in the body.
- Every other part is an **attachment**. That includes a `text/plain` part
  marked `Content-Disposition: attachment` (its lines are *not* mixed into the
  body) and inline images referenced by `Content-ID`.
- `multipart/*` container nodes are MIME structure and appear in neither list.

`filename` comes from the `Content-Disposition` `filename` parameter — including
RFC 2231 extended values such as `filename*=utf-8''...` — falling back to the
`Content-Type` `name` parameter. It is `""` when the part declares neither,
which is normal for inline images.

```python
import sys

from fast_mail_parser import parse_email, ParseError

# parse_email accepts both str and bytes; reading in binary mode is safest.
with open('message.eml', 'rb') as f:
    message_payload = f.read()

try:
    email = parse_email(message_payload)
except ParseError as e:
    print("Failed to parse email:", e)
    sys.exit(1)

print("Subject:", email.subject)
print("Date:", email.date)

# headers is a dict[str, list[str]]: every occurrence of a repeated header is
# kept, in the order it appeared. Single-valued headers are one-element lists.
for name, values in email.headers.items():
    for value in values:
        print(f"{name}: {value}")

# So a delivery path stays intact -- for Received, the first entry is the most
# recent hop.
for hop in email.headers.get("Received", []):
    print("Received:", hop)

# text_plain and text_html are lists of strings (one entry per matching part).
for body in email.text_plain:
    print("Plain text body:", body)

for body in email.text_html:
    print("HTML body:", body)

# attachments is a list of PyAttachment objects.
for attachment in email.attachments:
    print("Attachment:", attachment.filename)
    print("  mimetype:", attachment.mimetype)
    print("  size:", len(attachment.content), "bytes")  # content is bytes
```

## Contributing

Pull requests are welcome. For major changes, please open an issue first to discuss what you would like to change.

Please make sure to update tests as appropriate.

See [CONTRIBUTING.md](https://github.com/namecheap/fast_mail_parser/blob/master/CONTRIBUTING.md) for how to build from source, run the tests, and the PR conventions (linting, CI, DCO sign-off).
