# Patched copy of `mailparse` 0.16.1

This directory is [mailparse 0.16.1](https://crates.io/crates/mailparse/0.16.1) as published,
with **two functions changed** and one dependency added. It is applied through
`[patch.crates-io]` in the root `Cargo.toml` (and `fuzz/Cargo.toml`), so `cargo` sees the
same crate name and version and every other dependency resolves exactly as before.
`memchr = "2.7.0"` was added to this crate's `[dependencies]` (MIT OR Unlicense, no
dependencies of its own).

## The changes

Both replace a byte-at-a-time loop over the whole message body with a vectorised search.
Both return exactly what the code they replace returned.

**1. `find_from_u8` in `src/lib.rs`** -- the search `parse_mail` runs for every MIME
boundary -- scanned byte by byte:

```rust
for i in ix_start..=ix_end {
    if line[i] == key[0] { /* compare the rest */ }
}
```

It now calls `memchr::memmem::find`. Same result: first occurrence of `key` at or after
`ix_start`, `None` when there is none.

**2. `decode_base64` in `src/body.rs`** stripped whitespace before decoding with
`body.iter().filter(|c| !c.is_ascii_whitespace()).cloned().collect()`: a test and a
bounds-checked push per byte. It now calls `strip_ascii_whitespace`, which finds
whitespace with `memchr` and copies the runs between whole -- one search and one memcpy
per 76-byte line in the common case. The set of bytes removed is unchanged (exactly
`u8::is_ascii_whitespace`: space, tab, LF, form feed, CR) and a unit test in `body.rs`
checks it against the original filter over every byte value.

`diff -r` against the registry copy shows exactly `src/lib.rs`, `src/body.rs`,
`Cargo.toml` and this file.

## Why

Sampling parses of `tests/data/large_message.eml` (767 KiB, four base64 attachments):
**96.5%** of a metadata-mode parse was in the boundary scan, and once that was fixed,
**77.7%** of a full parse was in the whitespace filter. Worse than slow, both loops' speed
depended on where the linker placed them: the same x86-64 instructions ran at half speed
when a loop straddled a 64-byte boundary. A rustc minor version (#120) and a
version-string bump (#204) each moved the loops and each read as a regression of up to
96% -- with zero change to the instructions executed.

Measured on this machine (interleaved A/B, Apple M4), original master to both patches:

| benchmark | before | after |
|---|---|---|
| `parse_email(mode="metadata")` | 0.365 ms | 0.034 ms |
| `parse_email` (full) | 1.094 ms | 0.281 ms |
| `parse_many` (8 x 767 KiB) | 9.082 ms | 2.177 ms |

## When this goes away

Both changes are upstream as [staktrace/mailparse#142](https://github.com/staktrace/mailparse/pull/142).
Once a mailparse release includes them:

1. bump `mailparse` in the root `Cargo.toml` to that release,
2. delete the two `[patch.crates-io]` sections and this directory,
3. drop the `vendor/mailparse/**/*` entry from `[tool.maturin] include` in `pyproject.toml`
   and the `vendored mailparse tests` step from the lint job,
4. run the benchmark gate: the numbers should not move.

Until then, any upstream mailparse release has to be merged into this copy by hand; the
diff to carry forward is the two functions above and the test.
