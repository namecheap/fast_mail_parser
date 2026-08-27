# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **`parse_email_tree(payload)`** returns the message's MIME tree with the
  structure intact, and **`walk(part)`** iterates it depth-first in the same order
  as the stdlib's `email.message.Message.walk` (#99). A pure addition:
  `parse_email` is unchanged.
  - Two things the flat projection cannot express. A `multipart/alternative`
    node's children are the plain and HTML renderings *of the same thing*, which
    `text_plain`/`text_html` cannot relate. And a `message/rfc822` part -- a
    bounce or forward -- is now parsed rather than opaque: `is_message` is `True`
    and the embedded message's own root is the part's single child, so its
    headers are reachable instead of being an attachment blob to re-parse.
  - Embedded nesting counts against the same recursion cap as multipart nesting.
  - Tree topology is asserted against the stdlib's `walk()` over the whole
    fixture and RFC corpus, which is the strongest correctness oracle available.

- An internal panic now raises `ParseError` instead of PyO3's `PanicException`
  (#102). This is not about memory safety -- PyO3 already catches panics at the
  boundary, so one never aborted the process -- it is about which `except` clause
  a panic lands in. `PanicException` derives from `BaseException`, so the
  `except Exception` wrapped around a pipeline's parse call did not catch it and a
  single crafted message could take a worker down. The panic payload is kept in
  the error message, so the bug stays diagnosable. A panic still means a bug in
  this crate, and the message says so.

### Performance

- `bytes` payloads are no longer copied before parsing (#96). Every payload used
  to be duplicated into Rust-owned memory first, so `parse_many` cost the whole
  batch's size again in copies while the caller still held the originals. Batch
  parsing of 16 x 0.75 MiB messages is **27% faster** as a result, and its time is
  now within measurement error of parsing each message individually -- the
  per-payload overhead is gone rather than reduced.
  - This also removes the reason to avoid `parse_many` for large messages. It used
    to be ~1.5x *slower* than a Python thread pool there, because of this copy;
    the two are now level, while `parse_many` stays ~12x faster for the many-small
    -messages case a mail pipeline actually has. See the README.
  - `str` payloads are still copied and cannot not be: under the limited API,
    obtaining UTF-8 from a `str` means asking CPython to encode it. Pass `bytes`
    for the fast path.

### Fixed

- A message whose header block is never terminated by a blank line no longer
  loses its first MIME part (#150). RFC 5322 ends the header block with an empty
  line; real mail sometimes omits it, and the stdlib names the defect
  `MissingHeaderBodySeparatorDefect`. Without that line the underlying
  `mailparse` keeps consuming the body as headers -- it stops only at a blank line
  and accepts a colonless line as a field name -- so on the message this was found
  on it swallowed the first MIME boundary, which left the first part's body before
  the *next* boundary: multipart preamble, discarded by definition. The
  `text/html` alternative was delimited normally and survived, so the message came
  back looking populated with its plain-text body silently gone, its boundary
  delimiter reported as a header key, and the first part's own headers merged into
  the message's (two `Content-Type` values).
  - The payload is now normalised before it reaches `mailparse`, by the stdlib's
    rule: a non-continuation line in the header block that cannot be a header
    field ends the header block, and the body starts there. `parse_email`,
    `parse_many` and `parse_email_tree` all apply it, so the flat and structural
    views cannot disagree about such a message.
  - A message that has its separator -- every other fixture in the corpus, and
    every well-formed message -- is parsed from the original bytes, unchanged.
  - `tests/data/invalid_message.eml` is consequently no longer excluded from the
    stdlib-parity, MIME-tree and `parse_many` corpora. The two parsers now agree
    on its topology, its header set (29 keys, not 31) and both of its bodies; what
    is left is the CRLF and unfolding divergence that every real message shows.
- `headers` keys are now in the order the header names first appeared in the
  message, stably across parses. They came from a Rust `HashMap`, whose iteration
  order is randomised per instance, and that order became the Python dict's
  insertion order -- so identical bytes produced a different key order on every
  parse (forty distinct orders in forty parses) and the message's own header order
  was unrecoverable. Dict *content* was unaffected, which is why it went unnoticed
  (#157, found by the fuzz harness on its first run).
- `parse_many(payloads, threads=0)` now raises `ValueError` instead of silently
  behaving as `threads=None`. Treating 0 as "the machine's default" hid caller
  bugs: `threads=os.cpu_count() - 1` on a one-core machine, or an unset config
  value, quietly got full parallelism. Pass `None` to ask for the default. A
  negative value already raised `OverflowError` at conversion.

## [0.7.0] - 2026-08-26

### Breaking

- **`attachments` now contains only real attachments.** Previously every node of
  the MIME tree was reported, so body parts and `multipart/*` container nodes
  appeared alongside genuine files — a one-image message yielded four entries,
  three of them phantoms with empty filenames and, for containers, empty content
  (#22). Bodies and attachments are now disjoint: `multipart/*` nodes are MIME
  structure and appear in neither list. Code that counted `len(attachments)`, or
  that filtered containers out by hand, will see different numbers.

- **Body-vs-attachment classification now follows RFC 2183** instead of the media
  type alone (#25). Two consequences, both previously wrong:
  - A `text/plain` or `text/html` part marked `Content-Disposition: attachment`
    is an attachment, and its content is no longer concatenated into
    `text_plain` / `text_html`. Previously such a part corrupted the body.
  - An inline text part carrying a `Content-Type; name` parameter stays in the
    body. Previously a `name` alone removed it, silently losing body text.

  Both shapes are common in Outlook-generated mail.

- **`headers` is now `dict[str, list[str]]`.** It was `dict[str, str]`, backed by
  a Rust `HashMap<String, String>`, so a repeated key kept only its **last**
  value. Every earlier `Received`, `DKIM-Signature`, `Received-SPF` and so on was
  silently discarded, which made delivery-path tracing and signature
  verification impossible (#12, #23). Each key now maps to every value it
  appeared with, in message order. Single-valued headers are one-element lists,
  so callers never branch on `str`-vs-`list`:

  ```python
  mail.headers["Received"]   # ['from mx1...', 'from mx2...', 'from mx3...']
  mail.headers["From"]       # ['sender@example.com']
  ```

  Migration: index the list — `mail.headers["From"]` becomes
  `mail.headers["From"][0]`, or `mail.headers.get("From", [""])[0]` to keep a
  missing-header fallback.

### Added

- **`PyMail.date_parsed`** — `date` resolved to a timezone-aware `datetime` in
  UTC, or `None`. Completes #98. It is a getter computed on access rather than a
  field built during parsing, so callers that never read it pay nothing.

  Note the failure mode this deliberately avoids: `mailparse::dateparse` returns
  `Ok(0)` for input it never actually parsed — its loop simply never advances
  state and the function still returns its initial `0` — so a naive wrapper would
  report `not a date` as **1970-01-01** instead of `None`. Silently wrong is
  worse than absent, so a date is only trusted when a recognized month token is
  present, which the parser cannot reach a real result without. A legitimate
  epoch-0 date (`Thu, 01 Jan 1970 00:00:00 +0000`) still parses.

- **A migration guide, `docs/migrating.md`** — covering the 0.6.x -> 0.7.0
  breaking changes and the move from the stdlib `email` module, plus an honest
  list of what this library deliberately does not do (building or mutating
  messages, `Message` compatibility, header mutation). Every Python snippet in it
  is extracted and executed against the built wheel by
  `tests/test_docs_snippets.py`, in document order in one shared namespace, so a
  snippet that drifts from the API fails CI rather than misleading a reader
  (part of #103).

- **Typed address fields on `PyMail`:** `from_` (a `PyAddress` or `None`) plus
  `to`, `cc`, `bcc` and `reply_to` (lists of `PyAddress`), each with
  `display_name: str | None` and `address: str`. `mailparse` already parsed these
  and the binding layer discarded them, leaving every consumer to dig through
  `headers` and hand-roll RFC 5322 address parsing — display names, quoted
  strings containing commas, groups, comments — which is the classic thing to get
  wrong in a one-off regex (#98).

  RFC 5322 groups are flattened to their member mailboxes. An address header that
  does not parse yields an empty list (or `None`) rather than raising, so a
  malformed `To:` cannot fail an otherwise good message; the raw value stays in
  `headers`. Parsing goes through `addrparse_header` rather than the string form,
  so an RFC 2047 display name that decodes to something containing a comma or
  angle bracket cannot corrupt the address split.

- `PyAttachment.content_id` — the part's `Content-ID` with angle brackets
  stripped, or `None`. RFC 2392 `cid:` URLs reference that bracket-less form, so
  resolving the inline images an HTML body points at is now a dictionary lookup.
  It was previously impossible: the value was parsed and discarded at the FFI
  boundary (#98).

- `PyAttachment.disposition` — the raw `Content-Disposition` token, typically
  `"inline"` or `"attachment"`, or `None` when the part declares no such header.
  An absent header is reported distinctly from an explicit `inline`, which
  mailparse's parsed value alone cannot express since it defaults to `Inline`
  (#98).

- **`parse_many`** — batch parsing in one FFI call, in parallel, results in input
  order (#96). Each slot is a `PyMail` or a `ParseError` *instance*, returned
  rather than raised, so one malformed message does not cost the caller the rest
  of the batch; `raise_on_error=True` restores fail-fast. `threads` caps the
  worker count. The GIL is released for the whole batch rather than per message.

  Implemented on `std::thread::scope` with a shared atomic cursor rather than a
  thread-pool dependency: it adds nothing to the lockfile or the licence
  allowlist, and the cursor gives dynamic work distribution, which is the
  property that matters when message sizes are uneven — static chunking stalls a
  worker that draws several large messages.

  Note that every parsed message is materialised before returning, so large
  workloads should be chunked at the caller.

- **A `ParseError` hierarchy** (part of #100): `HeaderParseError`,
  `MimeStructureError` and `DecodeError`, all inheriting from `ParseError` so
  `except ParseError` keeps catching everything. Failures are categorised where
  they occur, so a caller can distinguish "this is not an email" from "one
  attachment's base64 is broken" — the second usually means an otherwise
  plausible message with one corrupt part, which is worth routing differently.
  Existing tests for the oversized-input, MIME-depth and broken-encoding paths
  now assert the specific subtype.

- **An honest cross-library benchmark table** in the README, comparing
  fast_mail_parser, mail-parser and the stdlib `email` module on *equivalent
  work* — each asked for the same result, with a "work performed" column so the
  comparison can be checked rather than trusted. Completes #103.

  This corrected a mislabelled claim. The long-standing "~8x faster than
  mail-parser" figure came from a benchmark calling `MailParser.from_string`,
  which never invokes `.parse()` — so it timed a lazy structural scan, not
  mail-parser's own logic. The claim turned out not to be inflated (that call
  dominates the cost anyway), but it was measuring the wrong thing. The published
  ratios now state the machine they came from, and note that the same comparison
  yields 5.25x/6.42x on arm64 versus 8.50x/10.01x on CI's x86_64.

  Regenerate with `make bench-table`; CI also renders the table into the
  benchmark job summary on every run.

- **A differential compatibility suite against the stdlib `email` module**
  (`tests/test_stdlib_parity.py`) plus `docs/compatibility.md` (part of #103).
  Both parsers run over the whole fixture corpus and are compared on nine
  dimensions; a mismatch fails CI unless it is a declared, explained divergence,
  and a declared divergence that stops occurring fails too — so the document
  cannot drift from the code in either direction.

  The corpus now matches the stdlib **byte for byte**, attachment payloads
  included, on everything except five documented differences (body line endings
  preserved rather than normalised to LF being the one most likely to bite) and
  one case where this library is *more* correct: the stdlib surrogate-escapes raw
  UTF-8 in address headers (RFC 6532) where this library decodes it.

### Changed

- Bumped the benchmark baseline `mail-parser` 3.15.0 -> 4.6.4 (test dependency
  only). The published comparison table names the version it was measured
  against, so it is regenerated alongside.

- CI: the benchmark gate selects its two benchmarks by exact name rather than by
  substring. A second benchmark whose name contained `fast_mail_parser` — such as
  one added for the comparison table — previously made the selection ambiguous
  and failed the gate instead of being ignored.

- CI: `cargo deny` now runs, enforcing the supply-chain policy in `deny.toml`
  (advisories, licence allowlist, bans, source allowlist). The file had declared
  all of it since it was added with nothing enforcing any of it, and had drifted:
  `0BSD` was missing from the allowlist while `mailparse` -- the crate this
  library is built on -- and its `quoted_printable` dependency are both 0BSD, so
  the policy as written rejected the core dependency. `0BSD` is now allowed, with
  the reasoning recorded next to it (#131).

- The crate now declares `license = "Apache-2.0"` as an SPDX expression rather
  than only pointing at the licence file, so tooling can classify it.

- Bumped the pinned `encoding_rs` 0.8.30 -> 0.8.35 (lockfile only; `charset`
  already allowed it via `^0.8.22`). 0.8.30 dates from 2021 and this crate does
  the charset decoding for every text part, i.e. it runs on untrusted input.
  0.8.30 also declared only `license-file = "COPYRIGHT"` with no SPDX `license`
  field, which registries and licence tooling report as non-standard; 0.8.35
  declares `(Apache-2.0 OR MIT) AND BSD-3-Clause` properly.

- CI: `ruff.toml` targeted `py39` while `requires-python` is `>= 3.11`, which
  silently narrowed the pyupgrade rules — the lint reported "All checks passed"
  while four findings sat waiting at the correct target. Corrected, and the
  findings fixed (test files only; no library change).

- **CI: the benchmark gate now compares against the base revision** instead of
  gating an absolute ratio against pure-Python `mail-parser`. Both revisions are
  built and measured in the same job, so between-runner variance cancels.

  The old gate was measurably unreliable. Four consecutive runs of the same
  binary in one job spread **0.3%** (1.895-1.902 ms), while the same source
  across jobs spread **26%** (1.885-2.378 ms) — and `mail-parser` barely moved
  (14.2-14.5 ms), so the two implementations do not scale together and the ratio
  moved with the runner's CPU. A 7.0x floor therefore sat inside the noise band:
  it failed honest PRs, and any floor loose enough to stop flaking would also
  have missed the ~26% regression class it existed to catch (#120).

  The absolute ratio is still reported and still gated, but only as a loose
  catastrophic-drift net (5.0x) far below the observed range. The regression
  threshold against the base is +7%, which is ~20x the measured within-job noise.

### Fixed

- `subject` and `date` are read from the parsed headers directly instead of back
  out of the collected header map, so they no longer inherit that map's
  representation and always reflect the first occurrence of their field (#28).

- `PyAttachment.filename` is read from the `Content-Disposition` `filename`
  parameter, including RFC 2231 extended values (`filename*=utf-8''...`),
  falling back to `Content-Type; name` as before. Attachments that declare a
  filename only via the disposition — which is what `email.message.EmailMessage`
  emits, and therefore most modern mail — previously reported `""`.

## [0.6.1] - 2026-08-26

### Changed

- The PyPI description now leads with the rename. A PyPI project description is
  immutable per release and is built from `Readme.md`, so 0.6.0's page opened
  with badges and a wall of benchmark output before mentioning the new name —
  anyone landing there saw neither the announcement nor how to install. The
  README now opens with the package name, the install command, and proof that
  the import path is unchanged, followed by a Quickstart. This release exists to
  publish that text; there is no code change.
- README links are absolute. Repo-relative links (`CHANGELOG.md`,
  `CONTRIBUTING.md`) render as broken links on PyPI, which serves this file
  outside the repository.

## [0.6.0] - 2026-08-26

### Changed

- **The distribution is now published as `fast-mail-parser-ng`.** Install with
  `pip install fast-mail-parser-ng`. The import path is unchanged — existing
  code keeps working as-is:

  ```python
  from fast_mail_parser import parse_email, ParseError
  ```

  Only the name in your requirements file changes. No code in this release
  differs from 0.5.0; the version bump signals that consumers must update how
  they install the package.

  The `fast-mail-parser` name on PyPI still points at an unmaintained 0.2.5
  from June 2022, published by the library's original author before he left
  Namecheap. We do not control that name: the PEP 541 transfer request
  ([pypi/support#11044](https://github.com/pypi/support/issues/11044)) has been
  open and unattended since 2026-06-13. Rather than block releases on that
  queue indefinitely, this repository publishes under a name we own. If the
  transfer is ever granted, `fast-mail-parser` will resume as an alias.
- Wheels are now built with a pinned Rust toolchain (1.97.1). rustc 1.98.0
  makes the parser ~26% slower, so the pin keeps 0.6.0's wheels as fast as
  0.5.0's (#119, tracked in #120).

## [0.5.0] - 2026-08-03

### Changed

- Wheels now target the CPython stable ABI (`cp311-abi3`, via the
  `pyo3/abi3-py311` feature): a single wheel per platform supports every
  CPython ≥ 3.11, including versions released after the build. New CPython
  minors no longer require a repo change or a new release for installability
  (#14, #15, #101). The abi3 build benchmarked ~11% *faster* than the
  version-specific build (min parse time, CPython 3.12, Apple Silicon), so no
  hybrid version-specific wheels are shipped.
- CI now builds one abi3 wheel and runs the full test matrix (CPython
  3.11–3.14) against that same wheel — the stable-ABI contract is verified,
  not assumed. The per-version publish matrix collapsed to one wheel per
  platform.

## [0.4.0] - 2026-06-12

### Breaking

- Dropped support for Python 3.7–3.10; the minimum supported version is now
  **3.11** (`requires-python >= 3.11`).
- `str` input to `parse_email` is now decoded as UTF-8 (lossless). Previously
  each code point was truncated to its low byte, corrupting non-ASCII input.
  Output for non-ASCII `str` therefore changes — pass `bytes` for exact control.
- Message bodies that fail to decode (e.g. invalid base64) now raise
  `ParseError` instead of silently returning an empty value.

### Changed

- Upgraded PyO3 0.16.6 → 0.29.0, resolving RUSTSEC-2025-0020 and
  RUSTSEC-2026-0177.
- Upgraded `mailparse` 0.15.0 → 0.16.1.
- Track the stable Rust toolchain and declare the MSRV (`rust-version = 1.83`).
- Faster string-input parsing via a UTF-8 fast path.

### Added

- Support for CPython 3.13 and 3.14.
- Denial-of-service hardening: input-size cap (100 MiB) and MIME
  recursion-depth cap (256), both surfaced as `ParseError`.
- Public API contract tests, an RFC-feature `.eml` corpus, round-trip
  correctness tests, and an empty-field sentinel test.
- `CONTRIBUTING.md` with build-from-source and testing instructions.

### Security

- Fixed the lossy `str`→bytes conversion that corrupted non-ASCII input.
- Added untrusted-input DoS guards (input-size and recursion-depth caps).
- Hardened CI: PR-gated matrix, blocking `cargo audit`, SHA-pinned actions,
  Dependabot, `cargo-deny`, OIDC Trusted Publishing, and removed real PII from
  test fixtures.

## [0.3.0]

Prior release (PyO3 0.16.6). See the Git history for details.

---

The package version is single-sourced from `Cargo.toml`'s `[package].version`.
`pyproject.toml` declares `dynamic = ["version"]`, so maturin reads the version
from `Cargo.toml` at build time. Bump the version in `Cargo.toml` only.

[Unreleased]: https://github.com/namecheap/fast_mail_parser/compare/v0.7.0...HEAD
[0.7.0]: https://github.com/namecheap/fast_mail_parser/compare/v0.6.1...v0.7.0
[0.6.1]: https://github.com/namecheap/fast_mail_parser/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/namecheap/fast_mail_parser/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/namecheap/fast_mail_parser/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/namecheap/fast_mail_parser/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/namecheap/fast_mail_parser/releases/tag/v0.3.0
