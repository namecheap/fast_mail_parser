# Compatibility with the stdlib `email` module

Every difference between this library and Python's `email` module, on the surface
both of them model.

This page is not hand-maintained prose. `tests/test_stdlib_parity.py` parses the
whole fixture corpus with both parsers and compares them on nine dimensions —
subject, `text_plain`, `text_html`, attachments (mimetype, filename, decoded
bytes), the header multiset, `from`, `to`, `cc`, and the date instant. A mismatch
fails CI **unless** it is listed below with a reason, and a reason listed here
that no longer occurs also fails. So the table cannot drift from reality in
either direction.

## Where they agree

Everything not listed further down — which includes the entire generated RFC
corpus (RFC 5322 plain, multipart alternative/mixed/related, nested multipart,
base64 and quoted-printable bodies, RFC 2047 encoded subjects, RFC 2231 encoded
filenames, RFC 6532 UTF-8 headers, 8bit bodies, folded headers, empty bodies)
matching **byte for byte**, attachment payloads included.

That is a recent state of affairs. Before the RFC 2183 classification fix, this
library reported every MIME node as an attachment and collapsed repeated headers
to their last value, so the structural dimensions diverged on nearly every
multipart message.

## Where they differ

### 1. Raw UTF-8 in headers — this library is more correct

| | |
| --- | --- |
| Fixture | `rfc6532_utf8_headers` |
| Dimension | `from` |
| stdlib | `('\udcd0\udc9e\udcd1\udc82…', '\udcd0\udcbe…@…')` — lone surrogates |
| this library | `('Отправитель', 'отправитель@пример.рф')` |

RFC 6532 permits raw UTF-8 in header values. `email` with `policy=default`
surrogate-escapes those bytes rather than decoding them, so `.addresses` yields
unusable lone surrogates. This library decodes them.

If you are migrating *to* the stdlib, this one will cost you.

### 2. `headers` returns raw values; the stdlib normalises structured ones

| | |
| --- | --- |
| Fixture | `attachment_message` |
| Dimension | `headers` |
| stdlib | `Date: Wed, 24 Apr 2019 10:05:02 +0200` |
| this library | `Date: Wed, 24 Apr 2019 10:05:02 +0200 (CEST)` |

`headers` gives values as they appeared, unfolded and RFC 2047-decoded, and
nothing more. The stdlib additionally parses structured headers and re-emits a
canonical form, dropping the RFC 5322 comment `(CEST)`.

Use `date_parsed` when you want an interpreted value; use `headers` when you want
what the sender actually wrote.

### 3. Address headers are re-serialised in `headers`

| | |
| --- | --- |
| Fixture | `large_message` |
| Dimension | `headers` |
| stdlib | `From: Example Sender <sender@example.com>` |
| this library | `From: "Example Sender"<sender@example.com>` |

The underlying `mailparse` crate re-emits an address header's display name
quoted and without the space before the angle bracket. The two are equivalent
per RFC 5322 but not textually identical.

**Prefer the typed `from_` / `to` / `cc` fields** over parsing `headers`
yourself; they are unaffected. This is arguably a wart rather than a decision,
and is a candidate to raise upstream.

### 4. Body line endings are preserved, not normalised

| | |
| --- | --- |
| Fixture | `valid_message` |
| Dimensions | `text_plain`, `text_html` |
| stdlib | `"…browser (…)\n\n\n** What's New?\n"` |
| this library | `"…browser (…)\r\n\r\n\r\n** What's New?\r\n"` |

The stdlib's text content manager normalises line endings to `LF`. This library
returns the body as it arrived, so a message transmitted with `CRLF` keeps
`CRLF`.

This is the divergence most likely to surprise you in practice, because it
affects every multi-line body from a real mail server. If you are comparing
strings or splitting lines, normalise first:

```python
body = mail.text_plain[0].replace("\r\n", "\n")
```

### 5. Folded headers unfold to a single space

| | |
| --- | --- |
| Fixture | `valid_message` |
| Dimension | `headers` |
| stdlib | `i=1; mx.google.com;       dkim=pass …` |
| this library | `i=1; mx.google.com; dkim=pass …` |

RFC 5322 folding whitespace is semantically a single space, which is what this
library emits. The stdlib preserves the original run of spaces from the
continuation lines. Both are valid unfoldings; they are not string-equal.

## Deliberately not modelled

The comparison covers what both libraries expose. It does not cover what only the
stdlib does, because this is a parser rather than a mail library: constructing or
mutating messages, serialising them back out, `Message`-compatible objects, or
header mutation. See the [migration guide](migrating.md) for that list.
