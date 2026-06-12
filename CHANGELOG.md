# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/namecheap/fast_mail_parser/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/namecheap/fast_mail_parser/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/namecheap/fast_mail_parser/releases/tag/v0.3.0
