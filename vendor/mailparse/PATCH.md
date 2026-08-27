# Patched copy of `mailparse` 0.16.1

This directory is [mailparse 0.16.1](https://crates.io/crates/mailparse/0.16.1) as published,
with **one function changed** and one dependency added. It is applied through
`[patch.crates-io]` in the root `Cargo.toml` (and `fuzz/Cargo.toml`), so `cargo` sees the
same crate name and version and every other dependency resolves exactly as before.

## The change

`find_from_u8` in `src/lib.rs` -- the search `parse_mail` runs for every MIME boundary --
scanned byte by byte:

```rust
for i in ix_start..=ix_end {
    if line[i] == key[0] { /* compare the rest */ }
}
```

It now calls `memchr::memmem::find`, which is vectorised with runtime CPU dispatch. Same
result: first occurrence of `key` at or after `ix_start`, `None` when there is none.
`memchr = "2.7.0"` was added to this crate's `[dependencies]` (MIT OR Unlicense, no
dependencies of its own). Nothing else differs from the published crate; `diff -r` against
the registry copy shows exactly these two files plus this one.

## Why

Sampling a metadata-mode parse of `tests/data/large_message.eml` (767 KiB, four base64
attachments) put **96.5%** of the time in that loop. Worse, its speed depended on where the
linker placed it: the same 88 x86-64 instructions ran at half speed when the loop straddled
a 64-byte boundary. A rustc minor version (#120) and a version-string bump (#204) each
moved the loop and each showed up as a "regression" of up to 96% on the metadata path --
with zero changes to the instructions being executed.

Measured on the fix (interleaved A/B, Apple M4, master vs this patch):

| benchmark | before | after |
|---|---|---|
| `parse_email(mode="metadata")` | 0.365 ms | 0.030 ms |
| `parse_email` (full) | 1.098 ms | 0.761 ms |
| `parse_many` (8 x 767 KiB) | 9.164 ms | 6.223 ms |

## When this goes away

The change is upstream as [staktrace/mailparse#142](https://github.com/staktrace/mailparse/pull/142).
Once a mailparse release includes it:

1. bump `mailparse` in the root `Cargo.toml` to that release,
2. delete the two `[patch.crates-io]` sections and this directory,
3. drop the `vendor/mailparse/**/*` entry from `[tool.maturin] include` in `pyproject.toml`,
4. run the benchmark gate: the numbers should not move.

Until then, any upstream mailparse release has to be merged into this copy by hand; the
diff to carry forward is the one function above.
