# Patched copy of `mailparse` 0.16.1

This directory is [mailparse 0.16.1](https://crates.io/crates/mailparse/0.16.1) as published,
with **three functions changed** (two via new modules) and two dependencies added, one of
them pinned. It is
applied through `[patch.crates-io]` in the root `Cargo.toml` (and `fuzz/Cargo.toml`), so
`cargo` sees the same crate name and version and every other dependency resolves exactly
as before. `memchr = "2.7.0"` (MIT OR Unlicense, no dependencies of its own) and
`base64-simd = "0.8"` (MIT, pulls `vsimd` and `outref`, both MIT) are added to this
crate's `[dependencies]`, and `quoted_printable` is pinned to `=0.5.1` rather than `0.5.0`
-- see "Why quoted_printable is pinned" below.

## The changes

Each replaces a byte-at-a-time pass over the whole message body, and each returns exactly
what the code it replaces returned.

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

   The same function then decoded the stripped buffer with
   `data_encoding::BASE64_MIME_PERMISSIVE`. It now tries `base64_simd::STANDARD` first and
   keeps `data_encoding` as the arbiter of everything the SIMD decoder turns down, which is
   sound because `STANDARD` accepts a strict *subset* on a whitespace-free buffer -- same
   alphabet, padding required by both, and `STANDARD` is additionally strict about `=`
   appearing mid-stream and about non-zero trailing bits, the two lenient cases this
   library has always accepted. So "SIMD accepts, `data_encoding` rejects" is empty, and
   every rejection still carries `data_encoding`'s own `DecodeError { position, kind }`.
   `simd_and_data_encoding_agree` in `src/body.rs` and the `base64_agreement` fuzz target
   hold that up.

3. **`decode_quoted_printable` in `src/body.rs`** delegated the whole body to
   `quoted_printable::decode(.., ParseMode::Robust)`, which makes three passes over it: a
   char-by-char copy into a `String` through a `filter_map`, a second walk with `lines()`
   and `trim_end()`, then a decode loop with one bounds-checked `push` per byte into a
   `Vec` that was never given a capacity. It now calls `qp::decode_robust` (`src/qp.rs`,
   new, plain `std` plus the `memchr` already here, no `unsafe`): one allocation sized to
   the input, `memchr` to find line breaks and escapes, and `extend_from_slice` for
   everything between them, so a line with no `=` in it is one copy. The RFC 2047
   encoded-word path in `src/header.rs` still uses the crate -- header-sized inputs, and
   different semantics (`_` -> space, trailing-whitespace restore).

`diff -r` against the registry copy shows exactly `src/bytescan.rs`, `src/qp.rs`,
`src/lib.rs` (two `mod` lines and one function), `src/body.rs` (two functions and a
`mod tests`), `Cargo.toml` and this file.

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

## Why quoted_printable is pinned

`=0.5.1`, not the `0.5.0` upstream asks for. 0.5.2 changed what Robust mode emits for a
body whose last line ends in a soft break -- `b"abc=\n"` decodes to `b"abc\r\n"` on 0.5.1
and `b"abc"` on 0.5.2 (`if filtered.ends_with('\n')` gained `&& add_line_break ==
Some(true)`). `src/qp.rs` reproduces **0.5.1**, because that is the version the root
`Cargo.lock` pins and therefore what this library ships.

A caret requirement made that a trap twice over. `cargo update` could change body *and*
encoded-word decoding with nothing in the suite to notice; and this crate's own
`cargo test` resolves its own lockfile, independent of the root one, so it pulled 0.5.2
while the extension linked 0.5.1 -- which meant the differential test below was comparing
the new decoder against a version that does not ship. The pin closes both. `Cargo.lock` is
unchanged by it: the root already resolved to 0.5.1.

## Why the two byte-search functions use different tools

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
2. keep `src/bytescan.rs` and `src/qp.rs`, both `mod` lines, the three call sites, the
   `decode_base64` fast path with its `mod tests`, both `memchr` and `base64-simd` in
   `Cargo.toml`, and the `=0.5.1` pin on `quoted_printable`;
3. bump the version in this copy's `Cargo.toml` and the `mailparse = "..."` requirement in
   both root manifests together, since `[patch]` only applies when the patched version
   satisfies the requirement;
4. run this copy's own suite (the lint job does), then the benchmark gate.
