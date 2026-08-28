# Patched copy of `mailparse` 0.16.1

This directory is [mailparse 0.16.1](https://crates.io/crates/mailparse/0.16.1) as published,
with **one module added and two functions changed to use it**. No dependency is added. It
is applied through `[patch.crates-io]` in the root `Cargo.toml` (and `fuzz/Cargo.toml`), so
`cargo` sees the same crate name and version and every other dependency resolves exactly
as before.

## The change

`src/bytescan.rs` (new) scans a machine word at a time instead of a byte at a time, in
plain `std` with no `unsafe`: `usize` chunks via `chunks_exact` + `from_le_bytes`, the
exact `haszero` / `hasless` word tests (Anderson, *Bit Twiddling Hacks*) to decide whether
a word needs a closer look, and `trailing_zeros` to jump to a hit rather than rescan.

1. **`find_from_u8` in `src/lib.rs`** -- the search `parse_mail` runs for every MIME
   boundary -- scanned byte by byte. It now calls `bytescan::find`, which tests four words
   per branch for the key's first byte and compares the rest only at candidates. Same
   result: first occurrence of `key` at or after `ix_start`, `None` when there is none.
2. **`decode_base64` in `src/body.rs`** stripped whitespace with
   `iter().filter(|c| !c.is_ascii_whitespace()).cloned().collect()`: a test and a
   bounds-checked push per byte. It now calls `bytescan::strip_ascii_whitespace`, which
   skips any word with no byte below `0x21`, walks the mask bits of a word that has one,
   re-checks each candidate with `is_ascii_whitespace` (so `0x0B` and the other control
   bytes are kept, as before), and copies the runs between in one piece.

`bytescan::tests` compares both against the loops they replace over a generated corpus at
every alignment and over every byte value. `diff -r` against the registry copy shows
exactly `src/bytescan.rs`, `src/lib.rs` (a `mod` line and one function), `src/body.rs`
(one function) and this file.

## Why

Sampling parses of `tests/data/large_message.eml` (767 KiB, four base64 attachments):
**96.5%** of a metadata-mode parse was in the boundary scan, and once that was fixed,
**77.7%** of a full parse was in the whitespace filter. Worse than slow, both loops' speed
depended on where the linker placed them: the same x86-64 instructions ran at half speed
when a loop straddled a 64-byte boundary. A rustc minor version (#120) and a
version-string bump (#204) each moved the loops and each read as a regression of up to
96% -- with zero change to the instructions executed.

Interleaved A/B on an Apple M4, original master to this copy:

| benchmark | before | after |
|---|---|---|
| `parse_email(mode="metadata")` | 0.365 ms | 0.030 ms |
| `parse_email` (full) | 1.094 ms | 0.240 ms |
| `parse_many` (8 x 767 KiB) | 9.082 ms | 1.973 ms |

A first version used `memchr` (PRs #213, #214). Upstream declined it for adding a
dependency, so this dependency-free version replaced it (it measured within noise on the
metadata paths and 3-11% faster on the decoding paths, because the strip is one pass).

## Upstream, and when this goes away

The same change is proposed upstream as
[staktrace/mailparse#142](https://github.com/staktrace/mailparse/pull/142) (revised
2026-08-28 without the dependency). If a mailparse release includes it:

1. bump `mailparse` in the root `Cargo.toml` and `fuzz/Cargo.toml` to that release,
2. delete the two `[patch.crates-io]` sections and this directory,
3. drop the `vendor/mailparse/**/*` entry from `[tool.maturin] include` in
   `pyproject.toml` and the `vendored mailparse tests` step from the lint job,
4. run the benchmark gate: the numbers should not move.

Until then, each upstream mailparse release is a hand-merge into this copy:

1. `diff -r` the new release against the previous one (both under
   `~/.cargo/registry/src/*/mailparse-<version>/`) and apply that diff here -- not the
   other way round, or the two functions revert;
2. keep `src/bytescan.rs`, the `mod bytescan;` line and the two call sites;
3. bump the version in this copy's `Cargo.toml` and the `mailparse = "..."` requirement in
   both root manifests together, since `[patch]` only applies when the patched version
   satisfies the requirement;
4. run this copy's own suite (the lint job does), then the benchmark gate.
