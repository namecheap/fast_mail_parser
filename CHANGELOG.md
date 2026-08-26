# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

### Fixed

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
