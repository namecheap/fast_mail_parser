# fast_mail_parser

![Test](https://github.com/namecheap/fast_mail_parser/workflows/Test/badge.svg)
[![PyPI version](https://badge.fury.io/py/fast-mail-parser.svg)](https://badge.fury.io/py/fast-mail-parser)
[![Downloads](https://pepy.tech/badge/fast-mail-parser)](https://pepy.tech/project/fast-mail-parser)

> ## 📦 Back on `fast-mail-parser`
>
> ```bash
> pip install fast-mail-parser
> ```
>
> **On `fast-mail-parser-ng`?** That name carried releases 0.6.0–0.7.0 while the
> original was owned elsewhere. Ownership has since been transferred, so releases
> are published under the original name again. Change the name in your
> requirements file and nothing else — no code edits, same API:
>
> ```diff
> - fast-mail-parser-ng
> + fast-mail-parser
> ```
>
> **Your code does not change either way.** The import path has always been
> `fast_mail_parser`:
>
> ```python
> from fast_mail_parser import parse_email
> ```
>
> `fast-mail-parser-ng` is archived on PyPI: the versions published under it stay
> installable, so nothing pinning them breaks, but it takes no further releases.
> See [The name](#the-name).

A very fast Python library for parsing `.eml` files. It is built on the Rust
[mailparse](https://github.com/staktrace/mailparse) crate via
[pyo3](https://github.com/PyO3/pyo3), and parses roughly **5–10x faster** than
pure-Python implementations, depending on the CPU — see
[Benchmark](#benchmark) for the measured spread and how to reproduce it.

## Quickstart

```bash
pip install fast-mail-parser
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

## The name

Releases are published as **`fast-mail-parser`**, and the import path is
`fast_mail_parser`.

There was an interruption worth explaining, because two names exist on PyPI. The
`fast-mail-parser` project belonged to an account this project no longer
controlled and was frozen at an unmaintained **0.2.5 from June 2022**; only a
project owner can publish to a name, so fixes could not reach it. A PEP 541
transfer request sat unattended for months. Rather than hold releases behind that
queue, 0.6.0 through 0.7.0 shipped as `fast-mail-parser-ng`, with the import path
deliberately unchanged so the switch cost one line in a requirements file.

Ownership has since been transferred directly, so releases go back to the
original name from 0.8.0 on, and `fast-mail-parser-ng` is **archived** on PyPI:
read-only, taking no further releases. The three versions published under it stay
installable, so nothing pinning them breaks — archiving marks the project as
finished rather than removing anything. If you are on it, change the name in your
requirements file; there is nothing else to do.

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

All three libraries asked for the **same result** — subject, both body lists, and
attachments with their payloads decoded — on the same message:

| Library | Work performed | Min time | Relative |
| --- | --- | --- | --- |
| **fast_mail_parser** | parse + decode bodies + decode attachments | 2.09 ms | 1.00x |
| mail-parser | `from_string` + `.parse()` + read attributes | 13.45 ms | 6.44x |
| stdlib `email` | `message_from_bytes` + walk + `get_content` / `get_payload` | 17.93 ms | 8.59x |

Corpus: `tests/data/large_message.eml` (multipart/mixed, 6 MIME parts, 2 base64
attachments). CPython 3.12.14 on Linux x86_64 (GitHub Actions `ubuntu-latest`),
mail-parser 4.6.4, minimum of 31+ rounds.

**These ratios move with the hardware, so treat them as a magnitude rather than a
constant.** An earlier run of this same comparison on a faster CI runner recorded
8.50x and 10.01x rather than 6.44x and 8.59x, and an Apple Silicon laptop gives
5.25x and 6.42x: the interpreted parsers and the Rust extension do not scale
together across CPUs. Regenerate the table for your own machine with
`make bench-table`, which prints its own methodology line; CI also renders it
into the job summary of every benchmark run.

Two things this table deliberately does *not* do:

- It does not quote the CI gate's numbers. That gate compares a revision against
  its base rather than against another library, precisely because absolute
  cross-implementation ratios are unstable between machines — they were observed
  to swing ~26% between CI runners while within-run noise was ~0.3%.
- It does not reuse the gate's mail-parser baseline, which measures
  `MailParser.from_string` alone. That call never invokes `.parse()`, so it is a
  stable number for regression detection but not a fair cross-library figure.

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
| `headers` | `dict[str, list[str]]` | Every header's values, in order; keys in wire order. |
| `attachments` | `list[PyAttachment]` | Non-body parts (see below). |
| `warnings` | `list[ParseWarning]` | Lossy repairs this parse made; empty means none (see below). |

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
in message order, so repeated fields survive. The keys are themselves in the
order the names first appeared in the message, and that order is stable across
parses:

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

### Parsing a batch

`parse_many` parses a whole batch in one call, in parallel, releasing the GIL for
the batch rather than per message:

```python
from fast_mail_parser import ParseError, parse_many

results = parse_many(payloads)              # list[str | bytes] in, results in input order
results = parse_many(payloads, threads=8)   # cap the workers; default is the machine's
                                            # threads=0 raises; use None for the default
```

Each slot is a `PyMail` **or** a `ParseError` instance — returned, not raised —
so one malformed message does not cost you the rest of the batch, and inputs zip
cleanly to outcomes:

```python
for payload, outcome in zip(payloads, parse_many(payloads)):
    if isinstance(outcome, ParseError):
        quarantine(payload, reason=str(outcome))
    else:
        index(outcome)
```

Pass `raise_on_error=True` to raise the first failure instead.

**Chunk large workloads.** Every parsed message is materialised before the call
returns, so a batch of ten thousand one-megabyte mails holds essentially all of it
decoded at once. Feed it in chunks of a few hundred rather than a whole mailbox.

#### When it helps

`parse_email` already releases the GIL, so a Python thread pool over it is
already parallel. **`parse_many` is not parallel-versus-serial** — both use every
core. What it removes is per-call overhead: one crossing into Rust for the whole
batch instead of one per message, and no Python future per message.

That overhead is a fixed cost *per message*, so what decides the comparison is
message size. Measured against `ThreadPoolExecutor(max_workers=4)` +
`parse_email` on a 4-vCPU runner, median of 3 rounds:

| Messages | Size each | `parse_many` | Thread pool | vs thread pool |
| --- | --- | --- | --- | --- |
| 2000 | 0.8 KB | 4.5 ms | 52.5 ms | **11.7x faster** |
| 16 | 768 KB | 14.1 ms | 14.2 ms | level (1.01x) |

Per message that is **2.3 µs** against **26.2 µs** for the small case: the ~24 µs
gap is the Python-side cost, and it does not grow with the message. At 768 KB the
parse itself costs ~880 µs and swamps it.

Dividing those out — ~24 µs of overhead per message against a parse that costs
roughly 1.1 µs per KB — the two break even near **20 KB**, and below it
`parse_many` pulls away:

| Message size | Expected advantage |
| --- | --- |
| 2 KB | ~6x |
| 10 KB | ~2.8x |
| 20 KB | ~2x |
| 100 KB | ~1.2x |

So **`parse_many` is the right default for a mail pipeline**, where messages are
usually a few KB, and it costs nothing at any size — the last row is a wash, not
a penalty.

That was not true before 0.8.0. `parse_many` used to copy every payload before
parsing began, which made it **1.5x slower** for large messages and put a real
trade-off here; the copy is gone
([#96](https://github.com/namecheap/fast_mail_parser/issues/96)), and with it the
reason to avoid `parse_many` for large mail.

The break-even point is not portable — the parse scales with cores while the
per-call overhead does not — so treat the second table as the shape of the
trade-off and measure your own mix if it matters.

### Metadata-only parsing

Scanning a mailbox to classify by sender, subject and attachment inventory does
not need the attachments decoded — and on a message that is mostly attachment,
decoding is nearly all of the work.

```python
from fast_mail_parser import parse_email

mail = parse_email(payload, mode="metadata")

mail.subject, mail.date_parsed, mail.headers      # identical to full mode
for part in mail.attachments:
    print(part.filename, part.mimetype, part.encoded_size)
```

`encoded_size` is the bytes the part occupies **in the message**, before
transfer-decoding — named for what it is, because metadata mode cannot know the
decoded size without doing the decode it exists to skip (base64 inflates by about
a third). In full mode the decoded size is `len(content)`.

It is **not an upper bound** on the decoded size, which is easy to assume and
wrong: quoted-printable emits a line break as CRLF, so a body of bare LFs decodes
*larger* than it was encoded. Decoding cannot more than double a part.

Two things metadata mode deliberately does not give you:

**No bodies.** There is no `text_plain`/`text_html` — not empty lists, absent. An
empty list cannot be told apart from "this message has no text part", so a sweep
counting bodyless messages would count every message. A missing attribute fails
loudly instead.

**No decode errors.** It never decodes, so a part with a broken
`Content-Transfer-Encoding` passes silently here and raises `DecodeError` in full
mode. Header errors are reported in both.

The type follows the mode, so nothing changes for callers of the default:

```python
parse_email(payload)                    # PyMail
parse_email(payload, mode="metadata")   # PyMailMetadata
```

If you want structure rather than an inventory, `parse_email_tree` is the API
that keeps it.

### The MIME tree

`parse_email` hands back a flat projection: bodies in one list, attachments in
another, `multipart/*` containers dropped. That is what most code wants, and it
throws away the shape of the message. `parse_email_tree` keeps it.

```python
from fast_mail_parser import parse_email_tree, walk

root = parse_email_tree(payload)

for part in walk(root):                    # depth first, stdlib `walk()` order
    print(part.content_type, len(part.content or b""))
```

Each node carries `content_type`, `headers` (same semantics as `PyMail.headers`),
`filename`, `content_id`, `disposition`, `children`, and `content` — the
transfer-decoded bytes of a leaf, or `None` for a container, whose body is only
its children with boundaries between them.

Two things the flat projection cannot express:

**Which body goes with which.** A `multipart/alternative` node's children are the
plain and HTML renderings *of the same thing*. Through `parse_email` they are one
entry in `text_plain` and one in `text_html`, with nothing to relate them.

**What is inside a bounce.** A `message/rfc822` part is an embedded message —
ubiquitous in bounce and abuse handling. `parse_email` reports it as one
attachment blob to re-parse by hand; here it is parsed, `is_message` is `True`,
and the embedded message's own root is the part's single child:

```python
bounced = next(p for p in walk(root) if p.is_message)
print(bounced.children[0].headers["Subject"])   # the original message's subject
```

Embedded nesting counts against the same recursion cap as multipart nesting, so
an onion of forwards cannot recurse further than a multipart tree can.

#### Which API when

| | |
| --- | --- |
| `parse_email` | You want the subject, the body text, the attachments. Most code. |
| `parse_email_tree` | The shape matters: forensics, bounce processing, deciding which alternative to render, anything that would otherwise reach for the stdlib's `walk()`. |

Both accept the same payloads and raise the same errors. `parse_email` is
unchanged by the tree API existing — it is a pure addition.

### Error handling

Failure comes in two shapes here, and they are reported differently. A parse that
cannot proceed raises; a parse that proceeds by repairing something records a
warning and returns. The exceptions are below, the warnings channel is the
section after it.

`parse_email` raises a subtype of `ParseError`, chosen by what actually went
wrong:

| Exception | Meaning |
| --- | --- |
| `HeaderParseError` | The header section could not be parsed — usually the input is not an email at all. |
| `MimeStructureError` | Malformed MIME structure, or a resource cap tripped: over 100 MiB of input, or nesting deeper than 256 levels. |
| `DecodeError` | A part's `Content-Transfer-Encoding` did not decode (bad base64, bad quoted-printable). |

A bug in the parser itself — an internal panic — raises the base `ParseError`
rather than PyO3's `PanicException`. That matters because `PanicException`
derives from `BaseException`, so it would slip past the `except Exception` around
a worker's parse call and take the process down; the message carries the panic
text so the bug can still be reported. No input is known to cause one.

All three inherit from `ParseError`, so existing code keeps working:

```python
from fast_mail_parser import DecodeError, ParseError, parse_email

try:
    mail = parse_email(raw)
except DecodeError:
    quarantine(raw)          # one part's encoding is broken
except ParseError:
    reject(raw)              # not parseable at all
```

The distinction is worth acting on: a `DecodeError` says one part of an otherwise
plausible message is corrupt, while a `HeaderParseError` usually says the bytes
were never an email.

### Parse warnings: the lossy-success channel

An exception is not the only way a parse can go wrong. Real mail is messier than
"valid" or "invalid", and this parser has always been best-effort in the middle:
a charset label it cannot resolve is decoded as us-ascii, an address header it
cannot parse yields no mailboxes, a `Date` it cannot read leaves `date_parsed`
at `None`, and a header block that was never closed is resynced before parsing.
Every one of those returns a result. None of them used to say so.

`warnings` says so:

```python
from fast_mail_parser import parse_email

mail = parse_email(raw)

if mail.warnings:
    for warning in mail.warnings:
        print(warning.kind, warning.part_path, warning.detail)
    quarantine(raw)          # something in here was patched up
else:
    classify(mail)           # pristine
```

**The empty list is the contract.** `warnings == []` means the parser repaired
nothing, which is what makes it worth checking: a spam classifier or a forensic
tool can route everything else to review instead of deciding on content that was
silently mended. Best-effort parsing is not new — being able to tell that it
happened is.

Each `ParseWarning` carries three strings:

| Field | Meaning |
| --- | --- |
| `kind` | A stable token to match on. The set grows, so treat an unfamiliar kind as "something was repaired" rather than as impossible. |
| `part_path` | Where the affected part landed in the result — `"text_plain[0]"`, `"text_html[1]"` — or `""` when the warning is about the message as a whole. |
| `detail` | Prose for a log. Not a matching key: the wording is free to improve, `kind` is not. |

The kinds emitted today:

| `kind` | What was repaired | What you still get |
| --- | --- | --- |
| `charset-fallback` | The part declared a charset label that is not recognised, so its bytes were decoded as us-ascii — which turns every non-ASCII byte into `U+FFFD`. | The decoded text, lossy exactly where the replacement characters are. |
| `address-unparseable` | An address header did not parse (mailparse rejects an address with no `@`), so no mailboxes were reported for it. | `to`/`cc`/… empty, or `from_` as `None`, with the raw value still in `headers`. |
| `date-unparseable` | The `Date` header is not a date any parser here recognises. | `date` as the raw header string; `date_parsed` is `None`. |
| `unterminated-header-block` | The header block was not closed by an empty line (RFC 5322 2.1), so the separator was restored before parsing — the stdlib calls this `MissingHeaderBodySeparatorDefect`. | The whole message, parts included. Left unrepaired this used to lose a body part silently ([#150](https://github.com/namecheap/fast_mail_parser/issues/150)). |
| `transfer-decode-lossy` | A quoted-printable part contained an escape that is neither `=` plus two hex digits nor a soft line break, and robust decoding passed it through as literal text instead of failing. | The decoded text with the escape still in it, undecoded — `=ZZ` stays three characters where the sender meant one byte. |

`transfer-decode-lossy` deliberately does **not** report line-ending
canonicalisation. Robust decoding also turns a bare LF into CRLF, which a strict
decoder rejects too — but then most mail written with bare LFs would warn, and a
channel whose empty list means something cannot afford to cry wolf. The case worth
reporting is the one where the sender's intent is lost, not the one where the
bytes are merely normalised.

`part_path` is a locator into the returned `PyMail`, not MIME tree coordinates.
That is deliberate: `parse_email` hands back a flat projection, so a coordinate
naming structure it has already discarded would be a locator you could not
resolve. Index into the list it names and you have the affected value. When the
structure is what matters, `parse_email_tree` is the API that keeps it.

`parse_many` carries warnings the same way — one list per message, on each
`PyMail`:

```python
for payload, outcome in zip(payloads, parse_many(payloads)):
    if isinstance(outcome, ParseError):
        reject(payload)
    elif outcome.warnings:
        review(payload)
    else:
        accept(outcome)
```

### Strict mode

A validation pipeline usually wants the opposite trade: fail rather than accept a
repair. `strict=True` raises each of the conditions above instead of recording
it, using the same exception hierarchy:

```python
from fast_mail_parser import DecodeError, HeaderParseError, parse_email

try:
    mail = parse_email(raw, strict=True)      # nothing was repaired
except HeaderParseError:
    ...                                       # includes an unparseable address header
except DecodeError:
    ...                                       # includes a charset fallback or an unreadable Date
```

| `kind` | Raised as |
| --- | --- |
| `charset-fallback` | `DecodeError` |
| `date-unparseable` | `DecodeError` |
| `transfer-decode-lossy` | `DecodeError` |
| `address-unparseable` | `HeaderParseError` |
| `unterminated-header-block` | `MimeStructureError` |

Strict mode adds rejections; it never reclassifies. A message that parses
cleanly parses identically in both modes, and a message that fails outright
fails with the same type either way. The exception names the first repair and
counts them all — parse without `strict=True` to read the rest.

`parse_many(payloads, strict=True)` applies it per slot, so one repaired message
becomes that slot's exception rather than costing you the batch; add
`raise_on_error=True` to fail the whole batch on the first repair.

`strict=True` requires `mode="full"`. Combining it with `mode="metadata"` raises
`ValueError` rather than being ignored: that mode never reads the bodies, so the
strongest thing it could say is "nothing in the headers was repaired", and a flag
that means something weaker than it says is worse than one that is unavailable.
Metadata mode has no `warnings` attribute for the same reason — the same
reasoning that leaves `text_plain` absent from it rather than empty.

**What is not reported.** A broken quoted-printable body is repaired silently by
the decoder rather than reported by it — mailparse decodes quoted-printable in
its robust mode — so there is nothing this crate can observe without decoding
twice. That is why there is no `transfer-decode-lossy` kind, and why the honest
place to say so is here rather than in a list a reader would take as complete.

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
