# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0]

### Changed

- Upgraded PyO3 from 0.16 to 0.29, resolving RUSTSEC-2025-0020 and
  RUSTSEC-2026-0177 advisories.
- Added a fast path for string input parsing, improving throughput.

### Added

- Public API contract tests to guard the exposed interface.
- An RFC-feature `.eml` test corpus covering MIME and RFC 822 edge cases.

### Security

- Hardened CI: enforced `rustfmt` and `cargo audit`, added Dependabot and
  `cargo-deny` configuration, and pinned the audit cache.

---

The package version is single-sourced from `Cargo.toml`'s `[package].version`.
`pyproject.toml` declares `dynamic = ["version"]`, so maturin reads the version
from `Cargo.toml` at build time. Bump the version in `Cargo.toml` only.

[Unreleased]: https://github.com/namecheap/fast_mail_parser/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/namecheap/fast_mail_parser/releases/tag/v0.3.0
