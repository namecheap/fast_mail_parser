# Patched copy of `mailparse` 0.16.1

This directory is [mailparse 0.16.1](https://crates.io/crates/mailparse/0.16.1) as published,
with **two functions changed** (one via a new module) and two dependencies added. It is
applied through `[patch.crates-io]` in the root `Cargo.toml` (and `fuzz/Cargo.toml`), so
`cargo` sees the same crate name and version and every other dependency resolves exactly
as before. `memchr = "2.7.0"` (MIT OR Unlicense, no dependencies of its own) and
`base64-simd = "0.8"` (MIT, pulls `vsimd` and `outref`, both MIT) are added to this
crate's `[dependencies]`.

## The changes

Both replace a byte-at-a-time loop over the whole message body. Both return exactly what
the code they replace returned.

1. **`find_from_u8` in `src/lib.rs`** -- the search `parse_mail` runs for every MIME
   boundary -- scanned byte by byte. It now calls `memchr::memmem::find`. Same result:
   first occurrence of `key` at or after `ix_start`, `None` when there is none.
2. **`decode_base64` in `src/body.rs`** stripped whitespace with
   `iter().filter(|c| !c.is_ascii_whitespace()).cloned().collect()`: a test and a
   bounds-checked push per byte. It now calls `bytescan::strip_ascii_whitespace`
   (`src/bytescan.rs`, new, plain `std`, no `unsafe`): a word with no byte below `0x21`
   cannot contain whitespace and is skipped whole; for one that might, the exact `hasless`
   mask (Anderson, *Bit Twiddling Hacks*) says which bytes to look at, each re-checked with
   `is_ascii_whitespace` so `0x0B` and the other control bytes are kept, as before; runs
   are copied in one piece. Its tests compare it against the filter it replaces over a
   generated corpus at every alignment and over every byte value.

`diff -r` against the registry copy shows exactly `src/bytescan.rs`, `src/lib.rs` (a `mod`
line and one function), `src/body.rs` (one function), `Cargo.toml` and this file.

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
| `parse_email` (full) | 1.094 ms | 0.228 ms |
| `parse_many` (8 x 767 KiB) | 9.082 ms | 1.834 ms |

## Why a dependency is justified for the decode when it was not for the strip

The rule for this copy is *no degradation*, and for the strip that ruled the dependency
**out**: a dependency-free word-at-a-time version measured faster than the `memchr` one, so
there was nothing to buy. The decode is the opposite case. `data_encoding`'s loop is four
table lookups per three bytes and it is already at scalar peak on both CPUs tried; there is
no dependency-free variant that is "no slower", because the only lever left is wider lanes.
`base64-simd` uses AVX2/SSE4.1 with runtime detection on x86-64 and NEON on aarch64.

Measured on an Apple M4, interleaved, 8 rounds per side, pure-Python controls within 1.1%:
a full `parse_email` **0.254 -> 0.168 ms**, `parse_email_tree` 0.246 -> 0.159 ms,
`parse_many` (8 x 767 KiB) 1.989 -> 1.318 ms. Metadata and untouched-lazy modes never call
this function and are flat, as they must be.

Stated plainly: `base64-simd`'s last release is 2022-12 and it is `unsafe`-heavy SIMD.
`cargo deny` and `cargo audit` are the gates, and RustSec carried no advisory for it,
`vsimd` or `outref` when this landed. The fallback limits the blast radius of a *rejection*
bug to a slow path, but not that of a *wrong-bytes* bug -- which is what the differential
test and the fuzz target are for.

**Do not rewrite the call out of place into an in-place decode.** `decode_inplace` over the
stripped buffer reuses the strip's allocation and avoids a second one, so it reads strictly
better. Measured in this crate, built as the extension is built (`lto = true`,
`codegen-units = 1`), it made the whole parse **9% slower** where the out-of-place form
makes it **51% faster** -- same machine, same corpus, same decoder, and the two are within
10% of each other when benchmarked standalone. That is the code-layout sensitivity #204
describes, alive and well. Measure before changing the shape of this function.

## Why the two functions use different tools

Upstream declined a version of this change that used `memchr` for both (staktrace/mailparse#142:
no new external dependencies), so a dependency-free word-at-a-time version of both was
written and is what upstream is now offered
([staktrace/mailparse#143](https://github.com/staktrace/mailparse/pull/143)). This copy
takes the half of it that is strictly no slower here:

- The **strip** is the dependency-free version. It is one pass where the `memchr` version
  made two searches per run, and measured faster on both CPUs tried (Apple M4: -9 to -12%
  on the decoding paths; EPYC 7763 on the CI gate: -2.5 to -2.9%).
- The **byte search stays on `memchr`**. Three dependency-free variants went through the
  gate: four words per branch measured +13-14% on the metadata paths on an EPYC 7763, eight
  words (a cache line) +9-11% -- 6 us per 767 KiB. A word-at-a-time scan tops out below a
  32-byte AVX2 compare, and the rule for this copy is no degradation.

So if a mailparse release ever includes #143, switching this copy's search to it (and
dropping the directory) is a decision that costs about 10% on `mode="metadata"` on x86 and
nothing on the decoding paths. The removal steps for that case:

1. bump `mailparse` in the root `Cargo.toml` and `fuzz/Cargo.toml` to that release,
2. delete the two `[patch.crates-io]` sections and this directory,
3. drop the `vendor/mailparse/**/*` entry from `[tool.maturin] include` in
   `pyproject.toml` and the `vendored mailparse tests` step from the lint job,
4. run the benchmark gate and read the metadata rows with the number above in mind.

## Keeping this in sync

Until then, each upstream mailparse release is a hand-merge into this copy:

1. `diff -r` the new release against the previous one (both under
   `~/.cargo/registry/src/*/mailparse-<version>/`) and apply that diff here -- not the
   other way round, or the two functions revert;
2. keep `src/bytescan.rs`, the `mod bytescan;` line, the two call sites, the
   `decode_base64` fast path with its `mod tests`, and both `memchr` and `base64-simd` in
   `Cargo.toml`;
3. bump the version in this copy's `Cargo.toml` and the `mailparse = "..."` requirement in
   both root manifests together, since `[patch]` only applies when the patched version
   satisfies the requirement;
4. run this copy's own suite (the lint job does), then the benchmark gate.
