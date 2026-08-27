# fast_mail_parser

[![PyPI](https://img.shields.io/pypi/v/fast-mail-parser?logo=pypi&logoColor=white&label=PyPI)](https://pypi.org/project/fast-mail-parser/)
[![Python](https://img.shields.io/pypi/pyversions/fast-mail-parser?logo=python&logoColor=white)](https://pypi.org/project/fast-mail-parser/)
[![Wheels](https://img.shields.io/badge/wheels-abi3%20%C2%B7%2013%20platforms-blue?logo=rust&logoColor=white)](https://pypi.org/project/fast-mail-parser/#files)
[![Downloads](https://img.shields.io/pypi/dm/fast-mail-parser?color=blue)](https://pepy.tech/project/fast-mail-parser)
[![License](https://img.shields.io/github/license/namecheap/fast_mail_parser)](https://github.com/namecheap/fast_mail_parser/blob/master/LICENSE)

[![Test](https://img.shields.io/github/actions/workflow/status/namecheap/fast_mail_parser/test.yml?branch=master&label=test&logo=github)](https://github.com/namecheap/fast_mail_parser/actions/workflows/test.yml)
[![Deep fuzz](https://img.shields.io/github/actions/workflow/status/namecheap/fast_mail_parser/deep-fuzz.yml?branch=master&label=deep%20fuzz&logo=github)](https://github.com/namecheap/fast_mail_parser/actions/workflows/deep-fuzz.yml)
[![Publishing](https://img.shields.io/badge/PyPI-Trusted%20Publishing%20%2B%20PEP%20740%20attestations-3775A9?logo=pypi&logoColor=white)](https://github.com/namecheap/fast_mail_parser/blob/master/.github/workflows/publish.yml)
[![Changelog](https://img.shields.io/badge/changelog-keep%20a%20changelog-e05735)](https://github.com/namecheap/fast_mail_parser/blob/master/CHANGELOG.md)

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

Ownership has since been transferred directly, so releases go back to the original
name from 0.8.0 on, and `fast-mail-parser-ng` is being **archived** on PyPI:
read-only, taking no further releases. Its last release, **0.7.1**, is 0.7.0's code
with a deprecation notice for a description and nothing else — a signpost, not an
upgrade. The four versions published under that name stay installable, so nothing
pinning them breaks; archiving marks a project finished rather than removing
anything. If you are on it, change the name in your requirements file; there is
nothing else to do.

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

Every number here is from one CI run — the benchmark gate on
[#214](https://github.com/namecheap/fast_mail_parser/pull/214): CPython 3.12 on
Linux x86_64, a 4-vCPU GitHub Actions runner (an AMD EPYC 7763 that time), median
of three interleaved rounds unless a table says otherwise. The message is
`tests/data/large_message.eml`: multipart/mixed, 6 MIME parts, 767 KiB, 99%
of it two base64 attachments.

### Against other libraries

All three libraries were asked for the **same result** — subject, both body lists,
and attachments with their payloads decoded:

| Library | Work performed | Min time | Relative |
| --- | --- | --- | --- |
| **fast_mail_parser** | parse + decode bodies + decode attachments | 0.59 ms | 1.00x |
| mail-parser 4.6.4 | `from_string` + `.parse()` + read attributes | 14.81 ms | 25.08x |
| stdlib `email` | `message_from_bytes` + walk + `get_content` / `get_payload` | 20.23 ms | 34.27x |

This table is minimum-of-N (34+ rounds), which is what `make bench-table` prints
and what most library comparisons quote. Everything below is a median.

### By mode

The default parse decodes everything. The other modes exist for callers who
will not read everything, and are priced accordingly:

| Call | Median | vs default | What it skips |
| --- | --- | --- | --- |
| `parse_email(payload)` | 0.575 ms | 1.0x | nothing — the default |
| `parse_email(payload, mode="lazy")`, nothing read | 0.080 ms | **7.2x** | attachment decoding, until asked |
| `parse_email(payload, mode="lazy")`, every attachment read | 0.594 ms | 0.97x | nothing; same work, deferred |
| `parse_email(payload, mode="metadata")` | 0.051 ms | **11.3x** | every body and attachment |
| `parse_email_tree(payload)` | 0.556 ms | 1.03x | nothing — the full MIME tree |
| `parse_email_tree(payload, mode="lazy")`, nothing read | 0.077 ms | 7.5x | leaf decoding, until asked |
| `parse_email_tree(payload, mode="metadata")` | 0.054 ms | 10.6x | every leaf's content |

Reading every attachment through lazy mode costs about 3% over the default, so
**if you will read everything, use the default**; lazy mode is for when you will
not.

### Batches

| Batch | `parse_many` | Alternative | |
| --- | --- | --- | --- |
| 8 × 767 KiB, `threads=1` | 4.48 ms | — | 0.56 ms per message |
| 8 × 767 KiB, `threads=1`, `mode="metadata"` | 0.41 ms | — | **10.9x** the full batch |
| 16 × 767 KiB, all cores | 4.62 ms | 4.77 ms, `ThreadPoolExecutor` + `parse_email` | level (1.03x) |
| 2000 × 0.8 KB, all cores | 4.67 ms | 62.28 ms, `ThreadPoolExecutor` + `parse_email` | **13.3x** |
| 2000 × 0.8 KB, `mode="metadata"` | 4.20 ms | 4.67 ms, `mode="full"` | 1.11x |

The batch API removes per-call overhead — a fixed cost per message that
dominates small messages and vanishes into large ones; the GIL was already
released per call. Metadata mode removes decoding, which is proportional to
message size and so barely registers on small ones. They compose.

### Reading these numbers

**Ratios move with the hardware; treat them as a magnitude, not a constant.**
Before mailparse's two byte-at-a-time loops moved to `memchr` (see the
[changelog](https://github.com/namecheap/fast_mail_parser/blob/master/CHANGELOG.md)),
the cross-library table read 6.44x and 8.59x on one runner and 8.50x and 10.01x on
a faster one. An Apple M4 now gives 20.8x and 28.3x against this run's 25.1x and
34.3x. The interpreted parsers and the Rust extension do not scale together
across CPUs, and the runner fleet is not homogeneous — the same two binaries have
measured identically on one runner and 96% apart on another. Regenerate the
cross-library table on your own machine with `make bench-table`; CI renders it
into the job summary of every benchmark run.

**How CI measures.** The gate builds the revision *and* its base, alternates
measurement rounds between them and compares medians, with the pure-Python
libraries riding along as a noise floor: they cannot be affected by how the
extension was built, so a difference is believed only once it clears them.
Absolute cross-implementation ratios were observed to swing ~26% between
runners while within-run noise was ~0.3%, which is why the gate is relative and
this section quotes one run rather than averaging several.

**One thing the cross-library table does not do:** reuse the gate's own
mail-parser baseline, which measures `MailParser.from_string` alone. That call
never invokes `.parse()`, so it is a stable number for regression detection but
not a fair cross-library figure.

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

#### Batching in a mode

`parse_many` takes the same `mode=` as `parse_email`, and it means the same thing
per message. This is the mailbox sweep: the batch API removes the per-message
overhead and the mode removes the decoding, and they compose.

```python
from fast_mail_parser import parse_many

for mail in parse_many(payloads, mode="metadata"):     # list[PyMailMetadata | ParseError]
    print(mail.subject, [a.filename for a in mail.attachments])

results = parse_many(payloads, mode="lazy")            # list[PyLazyMail | ParseError]
```

The mode is uniform across the batch, which is what lets it pick the slot type;
`ParseError` instances still occupy failed slots, and `raise_on_error`, `threads`
and input order behave exactly as in the default mode.

On the attachment-heavy fixture, a batch of 8 × 767 KiB with `threads=1`, median
of three interleaved rounds on the CI runner: `mode="full"` 4.48 ms,
`mode="metadata"` 0.41 ms — **10.9x**, the same ratio the single-message mode gets,
now available to the batch. On 2000 × 0.8 KB it is 4.20 ms against 4.67 ms — a
1.11x edge, because small messages are mostly headers and there is little
decoding to skip.

`strict=True` with `mode="metadata"` raises `ValueError`, as it does on
`parse_email` — a mode that never reads the bodies cannot promise nothing in them
was repaired.

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

The type follows the mode, so nothing changes for callers of the default — and
all three entry points take the same three modes:

```python
parse_email(payload)                         # PyMail
parse_email(payload, mode="lazy")            # PyLazyMail
parse_email(payload, mode="metadata")        # PyMailMetadata

parse_many(payloads, mode="metadata")        # list[PyMailMetadata | ParseError]

parse_email_tree(payload, mode="metadata")   # PyMimePartMetadata
parse_email_tree(payload, mode="lazy")       # PyLazyMimePart
```

If you want structure rather than an inventory, `parse_email_tree` is the API
that keeps it.

### Deferred attachment decoding

The other high-volume shape is selective extraction: find the one PDF in a
mailbox, and decode only that. `mode="lazy"` reads the bodies as usual and
decodes each attachment on first access, caching the result.

```python
from fast_mail_parser import parse_email

mail = parse_email(payload, mode="lazy")

mail.subject, mail.text_plain, mail.warnings     # identical to full mode

for part in mail.attachments:
    print(part.filename, part.mimetype, part.encoded_size, part.is_decoded)

pdf = next(p for p in mail.attachments if p.mimetype == "application/pdf")
data = pdf.content        # decoded here, and only this one
assert pdf.content is data   # every later read is the same object
```

`encoded_size` is available before anything is decoded, which is what makes the
choice possible: picking an attachment must not require decoding all of them.
`is_decoded` says whether reading `content` is free or is about to cost a decode.

**On the attachment-heavy fixture** (767 KiB, 99% attachment by decoded content),
median of three interleaved rounds on the CI runner:

| | | |
| --- | --- | --- |
| `mode="metadata"` | 0.051 ms | decodes nothing |
| `mode="lazy"`, nothing read | 0.080 ms | bodies decoded, attachments deferred |
| `mode="full"` | 0.575 ms | the default |
| `mode="lazy"`, every attachment read | 0.594 ms | the same work, in a worse order |

So deferring saves about 86% when you were not going to decode everything, and
costs about 3% when you were. **If you are going to read every attachment, use the
default mode** — this one is for when you are not. Absolute times move with the
runner; the ratios are what to read.

Two things to know before choosing it:

**It trades memory for decoding.** The encoded bytes of every attachment are
retained until the message is dropped, and base64 is about 1.33x the size of what
it encodes — so a retained part costs *more* than the decoded bytes it avoids
producing. Right for one attachment out of twenty; wrong for all twenty.

**A `DecodeError` moves.** A part whose `Content-Transfer-Encoding` cannot be
decoded fails the whole parse in full mode, and fails on `content` here — so a
message with one broken attachment parses, and only that attachment raises. A
failed decode is not cached: the next read raises again.

`content` is thread-safe. Several threads reading it concurrently all get the same
object, the GIL is released for the decode so they overlap rather than serialise,
and `PyLazyAttachment` has no other mutable state. The cache is a `OnceLock` on
the Python object rather than anything shared or static, so the free-threading
audit's invariant — no shared mutable state in the parsing core — still holds.

`PyLazyAttachment` is a new type rather than a lazier `PyAttachment`: changing what
an existing attribute costs, and where it raises, is a change to a shipped
contract. `PyAttachment.content` is exactly what it was.

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

#### Walking without decoding

A full-mode tree decodes every leaf, which is the wrong bill for the thing the
tree is best at: walking a large message to pull one part out of it. `mode=`
takes the same three values here, and the shape of the tree is identical in all
three — only what a leaf's bytes cost changes.

```python
from fast_mail_parser import parse_email_tree, walk

root = parse_email_tree(payload, mode="lazy")

for part in walk(root):
    print(part.content_type, part.encoded_size, part.is_decoded)

pdf = next(p for p in walk(root) if p.content_type == "application/pdf")
data = pdf.content            # decoded here, and only this one
```

`mode="metadata"` decodes nothing *and retains nothing*: a node reports
`encoded_size` in place of `content` and there is no way to ask for the bytes.
That is the difference between the two — lazy mode keeps a copy of every leaf so
it can decode one later, metadata mode keeps none and is the cheaper sweep.

On the attachment-heavy fixture (767 KiB), median of three interleaved rounds on
the CI runner: full tree 0.556 ms, `mode="lazy"` with nothing read 0.077 ms,
`mode="metadata"` 0.054 ms — **7.2x** and **10.3x**.

Two things to know:

**A metadata node has no `content` at all**, not `content = None`. On a
`PyMimePart`, `content is None` means "this is a container" and only that; a mode
where it also meant "not decoded" would make the two indistinguishable. A missing
attribute fails loudly instead, exactly as `PyMailMetadata` omits `text_plain`.

**A `message/rfc822` body is still decoded, in every mode.** That body *is* the
embedded message, and parsing it is what gives the node children — so a tree that
deferred it would be deferring the structure, which is the one thing every mode
has to deliver eagerly. Its node therefore arrives with `is_decoded` already
`True`, and unlike `parse_email(mode="metadata")` a deferred tree *can* raise
`DecodeError` for such a part. Nothing else is decoded.

`walk` accepts a node from any mode and yields nodes of the same type.

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

`strict=True` requires a mode that reads the bodies, so `mode="full"` or
`mode="lazy"`. It means the same thing in both: lazy mode decodes every body part
exactly as full mode does and finds every repair full mode finds, so its
`warnings` is the same list — deferring attachment *content* changes when a
`DecodeError` surfaces and nothing about what was repaired.

Combining it with `mode="metadata"` raises `ValueError` rather than being ignored:
that mode never reads the bodies, so the strongest thing it could say is "nothing
in the headers was repaired", and a flag that means something weaker than it says
is worse than one that is unavailable. Metadata mode has no `warnings` attribute
for the same reason — the same reasoning that leaves `text_plain` absent from it
rather than empty.

**What is not reported.** Robust quoted-printable decoding also canonicalises
line endings, turning a bare LF into CRLF. A strict decoder rejects that too, but
reporting it would warn on most mail written with bare LFs, and a channel whose
empty list is its whole contract cannot cry wolf. `transfer-decode-lossy` covers
the case where the sender's intent is lost — an escape that is neither `=` plus
two hex digits nor a soft line break — not the case where bytes are merely
normalised.

Nothing else is knowingly unreported. If you find a lossy repair with no warning,
that is a bug worth filing: the empty list is only useful if it is exact.

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
