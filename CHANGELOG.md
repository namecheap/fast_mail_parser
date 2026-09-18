# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Lazy modes borrow the payload instead of copying every deferred part** (#239). Deferring
  a decode used to copy the part's encoded bytes out of the message, which is the opposite
  of what deferring is for: parsing a 96 MiB single-attachment message in `mode="lazy"` cost
  96 MiB *on top of* the payload the caller still held, and base64 is 1.33x what it encodes,
  so a retained part could cost more than the decoded bytes it avoided producing. A deferred
  part now keeps its offsets in the buffer it was parsed from, and the result keeps that
  buffer alive. Measured on a 96 MiB message with one unread attachment: peak RSS 192.3 MiB
  before, 96.2 MiB after. Counted in the core, on the attachment-heavy fixture: a lazy tree
  peaked at 798,111 bytes and now peaks at 14,332 -- the same as a metadata tree, because a
  range is not a copy -- and a flat lazy parse dropped from 790,993 to 23,398.

  **This changes the memory contract, and is the reason to read this entry.** A `PyLazyMail`
  attachment or a `PyLazyMimePart` leaf pins the payload it was parsed from for as long as it
  is reachable, so keeping one attachment out of a mailbox keeps that whole message rather
  than just the part. The pin is per message even in `parse_many`, so one slot never holds
  another's payload. A caller who wants the bytes without the message reads `content`, which
  is a decoded copy, and drops the attachment. Nothing about the values changes: every mode
  returns what it returned, including for a message whose header block had to be repaired
  (#150), where the offsets index the rebuilt copy that now travels with the result.

  One exception, invisible from Python: the leaves *inside* a `message/rfc822` node still hold
  copies. Their bytes were produced by decoding that node's body, so they are in no caller
  buffer to point into.

- **The parsing core is a crate, and has Rust tests for the first time** (#236). It was a
  `#[path]`-included file: the binding declared it as a module, and both fuzz targets
  reached across the tree to include the same source again under different cfg. Nothing
  could depend on it and nothing could test it directly, so a library whose reason to
  exist is parsing had zero Rust tests for the parsing -- every assertion had to go through
  Python. It is now `crates/fast_mail_parser_core`, a workspace member with `charset` and
  `mailparse` declared in its manifest alone, and the fuzz harness links it instead of
  copying it. Seven tests come with it, covering what is awkward to reach from Python: the
  input-size cap (no 100 MB object crosses the FFI boundary to test it here), the
  warning machinery's ordering, per-slot `parse_many` semantics, and metadata mode agreeing
  with the full parse on the envelope. `cargo tree -p fast_mail_parser_core` now *enforces*
  the no-PyO3 property the module docs used to merely claim. No behaviour change; the
  extension is functionally identical.

### Added

- **A Rust bench crate that measures the core directly** (#225). The pytest benchmarks are
  the right oracle for what the wheel costs a user and a poor one for asking where that
  cost is: every number carries the FFI crossing, the GIL and the construction of Python
  objects. `bench/` times the core itself, including two loops nothing could isolate
  before -- the whitespace strip and the base64 decode, which are most of a full parse on
  the large fixture. It also measures `b64-simd` against `b64-scalar`, which is what #228
  bought on its own rather than diluted through a whole parse: **1.66x** on an Apple M4
  (82.6 us against 136.8 us), with `bench/Cargo.lock` pinned to the versions the wheel
  ships and a lint check that keeps it that way. `cargo test` in `bench/` counts allocations per mode with a
  global allocator and turns the modes' documented memory claims into assertions: on the
  785 KiB fixture a full parse peaks at **1,060,757 bytes** and metadata mode at
  **11,929** -- 89x lower, because it copies no part bodies. One measured result corrects
  the intuition: a *lazy* tree holds more than a decoded one on a small
  attachment-bearing message, because lazy retains the encoded bytes and base64 is 4/3 of
  what it decodes to. A `profiling` cargo profile (release codegen plus line tables) makes
  the shipped build attributable to a line. No new runtime dependency; nothing under
  `src/`, `vendor/` or `fast_mail_parser/` changes and the wheel is byte-identical.

### Added

- **A dispatch-only PGO A/B** (#241). Profile-guided optimisation is easy to adopt on
  faith -- the compiler gets a real profile and the numbers usually move the right way --
  and this crate is a bad place for faith: code placement alone moves its benchmarks up to
  7.5% on the runners (#240, measured), and PGO's mechanism *is* rearranging code. So a 4%
  "win" here is indistinguishable from a lucky layout draw unless it is measured against
  that floor. `gh workflow run pgo-ab.yml` builds three wheels from one source and one
  toolchain -- plain, instrumented, and optimised with the profile that instrumented build
  produced from the whole test suite plus one pass of the benchmark bodies -- and measures
  plain against PGO interleaved on a single runner, in both orientations so a win
  announces itself as loudly as a loss. It fails the job if training wrote no `.profraw`,
  or if PGO produced a byte-identical extension, since both would report a reassuring 0%
  for the wrong reason. The decision rule is recorded in `CONTRIBUTING.md`: adopt only if
  the win clears the tolerance, exceeds the layout spread for the same revision, and
  reproduces on a second CPU. Nothing about the shipped wheels changes.

- **A layout A/B, so code placement can be measured instead of argued about** (#240).
  This crate has been bitten by placement four times: a rustc minor version moved the
  parse path 15-96% (#120), a package-version bump did the same for a byte-identical
  instruction stream (#204), and on 2026-09-17 three consecutive PRs failed the gate on
  `parse_qp_message` by +9.2%, +7.3% and +19.0% for changes that cannot reach
  quoted-printable decoding -- one of them touched only the thread scheduler. Until now
  the only remedy was prose telling the reader to re-run and hope for a different runner.
  `gh workflow run layout-ab.yml -f salts=4 -f rounds=3` now builds the same source K
  times differing only in a `-C metadata` salt -- what a version bump perturbs -- and
  measures them interleaved on one runner; `.github/scripts/layout_spread.py` reports the
  per-benchmark spread across salts, which is that revision's layout sensitivity on that
  CPU and the figure the gate's 7% threshold should be read against. Passing
  `-f rustflags=...` measures an alignment candidate as a second group, with a cost table.
  A benchmark is called layout-sensitive only if its spread clears both the pure-Python
  control floor and 3% -- below that a four-salt sweep cannot tell placement from the
  residual, and the rule is pinned by `tests/test_layout_spread.py`. Dispatch-only; no
  change to `src/`, `vendor/`, the shipped wheels or any build flag.

- **The vendored mailparse delta is a CI invariant** (#235). `vendor/mailparse` is
  upstream 0.16.1 with three functions changed, and upstream declined the change (#217),
  so the copy is a permanent carry and every mailparse release is a hand-merge into it.
  Ownership of that copy rested entirely on prose, and the prose had already drifted in
  three places. `vendor/mailparse/upstream.patch` is now the delta in machine-readable
  form, and `.github/scripts/check_vendored_mailparse.sh` re-applies it on every CI run --
  download the published crate, verify its sha256, apply the patch, `diff -r` against the
  vendored copy -- failing the lint job on any drift. It also asserts the two things the
  sync recipe is easiest to get wrong: that both root manifests require exactly the
  vendored version, and that `Cargo.lock` still records `mailparse` with no `source =`
  line, which is the signature of `[patch.crates-io]` being in effect. Nothing else could
  see this class of mistake: the vendored suite passes on unpatched upstream, because the
  patch does not change behaviour, so a half-applied hand-merge would have surfaced only
  as an unexplained 4-10x regression in the benchmark gate on some unrelated PR. The sdist
  check now also asserts `bytescan.rs`, `qp.rs` and the patch are shipped, and that a
  stray `target/` or `Cargo.lock` from running cargo in the vendored crate is not. No Rust
  source change; the wheel is byte-identical.

- **Benchmarks for what the batch API's shape actually costs** (#224). The suite could not
  answer three questions about `parse_many`: how it scales with `threads`, whether the
  atomic-cursor scheduler earns its keep on the uneven batches it was written for, and what
  aliasing costs now that payloads are borrowed rather than copied. Every batch measured so
  far was one message repeated. New, all informational:
  `parse_many_small_scaling[t1|t2|t4|all]` over 2000 small messages, and
  `parse_many_mixed[t1|all]` plus a metadata row over a seeded 200-message batch built from
  four fixtures at wildly different sizes. A `_distinct()` helper builds batches from
  separate buffers, and the default-thread large, small and small-metadata batches now use
  it. Measured on an Apple M4 (10 vCPU): the small batch scales **3.592 -> 2.396 -> 1.972 ms**
  at 1/2/4 threads and then *regresses* to 2.062 ms on all ten, so that path is bound by the
  serial marshalling under the GIL rather than by parsing; the uneven batch scales much
  better, **3.857 -> 1.407 ms (2.7x)**. Aliasing turned out not to matter on this machine --
  +0.6% at `threads=1`, 0.0% at default -- so switching the batches to distinct buffers
  moves no published figure. Test-only: the extension is byte-identical.

- **The benchmark gate judges more than one message shape** (#223). Every gated benchmark
  measured `large_message.eml` -- 767 KiB, 99% base64 attachment -- so the gate judged the
  decode path and nothing else. That is not hypothetical: #238's header work moved the
  small serial batch 23% while the gate's own benchmark moved 2%. Three gated benchmarks
  now cover the shapes it could not see: `parse_small` (the per-call floor on ~0.8 KB --
  FFI, header map, address and date parse), `parse_many_small_serial` (the same cost x2000,
  serial, in the milliseconds range), and `parse_rfc2047_headers` (a ~30 KB header block
  with encoded words throughout, the one path no other gated benchmark touches). Each
  asserts correctness once outside the timed call, including `warnings == []`, so none of
  them can be timing a repair. The RFC 2047 input is built in the benchmark module rather
  than committed to `tests/data/`, where every `.eml` is auto-enrolled in eight correctness
  suites. Quoted-printable coverage arrived earlier with #229. Test-only: the extension is
  byte-identical.

### Changed

- **A part's body is evaluated once, and plaintext bodies stop being copied** (#230).
  `get_body_encoded()` re-reads a part's headers to find its transfer encoding, and the
  full and lazy parsers each called it two or three times per part -- for the
  quoted-printable escape check, for the encoded size, and again inside `get_body_raw`.
  It is now called once and threaded to all three. A 7bit/8bit/binary body *is* its raw
  bytes, so `get_body_raw` was copying it into a `Vec` purely for the charset step to
  borrow back; those bodies now decode from the borrowed slice. Output is unchanged and
  attachment bytes are byte-identical. Measured on an Apple M4 (10 vCPU), 5 interleaved
  rounds, pure-Python controls within 2.4%: the quoted-printable fixture
  **0.143 -> 0.131 ms (-9.8%)**, a plain 8bit text body **0.022 -> 0.021 ms (-6.4%)**.


### Added

- **The benchmark gate judges more than one message shape** (#223). Every gated benchmark
  measured `large_message.eml` -- 767 KiB, 99% base64 attachment -- so the gate judged the
  decode path and nothing else. That is not hypothetical: #238's header work moved the
  small serial batch 23% while the gate's own benchmark moved 2%. Three gated benchmarks
  now cover the shapes it could not see: `parse_small` (the per-call floor on ~0.8 KB --
  FFI, header map, address and date parse), `parse_many_small_serial` (the same cost x2000,
  serial, in the milliseconds range), and `parse_rfc2047_headers` (a ~30 KB header block
  with encoded words throughout, the one path no other gated benchmark touches). Each
  asserts correctness once outside the timed call, including `warnings == []`, so none of
  them can be timing a repair. The RFC 2047 input is built in the benchmark module rather
  than committed to `tests/data/`, where every `.eml` is auto-enrolled in eight correctness
  suites. Quoted-printable coverage arrived earlier with #229. Test-only: the extension is
  byte-identical.

### Changed

- **Quoted-printable bodies decode a run at a time** (#229). The last transfer decoder in
  this library that still ran byte-at-a-time: `decode_quoted_printable` handed the whole
  body to the `quoted_printable` crate, which copies it char by char into a `String`
  through a `filter_map`, walks that again with `lines()` and `trim_end()`, then decodes
  with one bounds-checked `push` per byte into a `Vec` with no capacity. Bodies now go
  through `vendor/mailparse/src/qp.rs`: one allocation sized to the input, `memchr` for
  line breaks and escapes, `extend_from_slice` between them -- so a line with no `=` is a
  single copy. Output is byte-identical; the RFC 2047 encoded-word path still uses the
  crate. Nothing in the benchmark suite could see this before, because every fixture was
  base64 or 8bit, so `test__fast_mail_parser___parse_qp_message` is added over the one
  quoted-printable fixture (89,932 bytes of HTML, 8,122 of text) together with a
  metadata-mode control on the same message. Measured on an Apple M4 (10 vCPU), 5
  interleaved rounds, pure-Python controls within 1.4%: **0.218 -> 0.133 ms (-39%)**, with
  every other benchmark inside the noise floor. `quoted_printable` is now pinned to
  `=0.5.1`: 0.5.2 changed Robust-mode output for a body ending in a soft break, and the
  caret requirement let this crate's own tests resolve a different version than the
  extension links. `Cargo.lock` is unchanged.

- **The header table allocates half as much** (#238). `collect_headers` keyed its
  position map by a decoded `String`, so every header allocated its key twice --
  once for the map and once for the table. Latin-1 decoding is injective, so the raw
  key bytes are exactly the same equivalence, case-sensitive grouping included, and
  both containers are now sized from the header count up front. Alongside,
  `disposition_token` tested for `Content-Disposition` with `get_first_value`, which
  normalised a value it dropped on the next line; `get_first_header` answers the same
  question with the same case-insensitive first-match semantics and no tokenizer.
  Measured on an Apple M4 (10 vCPU), 5 interleaved rounds, pure-Python controls within
  1.7%: 2000 x 0.8 KiB serial **4.048 -> 3.839 ms (-5.2%)**, the same batch in metadata
  mode **3.622 -> 3.442 ms (-5.0%)**, and `mode="metadata"` on the quoted-printable
  fixture 0.022 -> 0.021 ms. No output change.

- **`headers` is built once per object and shared, and every result class is `frozen`**
  (#231). Six classes expose `headers`, and each rebuilt a whole Python `dict` on every
  read -- one `PyDict`, a `str` per key, a `list` per key, a `str` per value, about ninety
  objects for a typical message -- on an attribute callers reasonably treat as stored and
  read several times. The README's own idiom reads it twice. All six now share one
  `Headers` type holding a `OnceLock<Py<PyDict>>`: the first read builds the dict, every
  later read returns that same object, and a parse that never touches `headers` builds
  nothing. Separately, the eleven `#[pyclass]`es are now `frozen` -- none has a
  `&mut self` method or a setter, so PyO3 was running an atomic compare-exchange borrow
  flag on every getter call to guard mutation that cannot happen. Measured on an Apple M4
  (10 vCPU), 5 interleaved rounds, pure-Python controls within 2.0%: three header lookups
  on a parsed message **2.416 -> 0.073 us, 33x**, and `attachment_reread` 0.083 -> 0.042 us
  from `frozen` alone. Parsing benchmarks are unaffected -- nothing here touches the decode
  loops.

  **Observable change:** `mail.headers is mail.headers` is now `True`, so edits to the
  returned dict persist across later reads *of that object*. The parse behind it does not
  move -- `subject`, `from_` and the rest are unaffected, and re-parsing gives the original
  headers back. `docs/migrating.md` said `headers` was "a plain dict snapshot"; it now says
  to treat it as read-only and to copy it if you need one you can edit.

- **`parse_many` sizes its workers by bytes, and the calling thread is one of them**
  (#232). Workers were capped on message count alone, so a sixteen-message fetch page --
  the shape a mail pipeline produces most often -- spawned a thread per message to parse
  about a microsecond each. Creating and joining an OS thread costs 15-40 us against a
  parse of roughly 1.1 us per KB, so `parse_many(page)` was **2.2x slower** than
  `parse_many(page, threads=1)`: the opposite of what the README promised. Workers are now
  also capped at one per 64 KiB of input, so a batch with less work than that runs inline;
  the calling thread runs the claim loop instead of blocking on joins, one fewer spawn per
  call; and the default parallelism is probed once per process instead of re-reading
  `/proc/self/cgroup` and the cgroup `cpu.max` files on every call. `threads=` remains an
  upper bound and is never raised by the gate. Results, ordering, per-slot errors,
  `raise_on_error`, `strict` and `mode=` are unchanged. Measured on an Apple M4 (10 vCPU),
  5 interleaved rounds, pure-Python controls within 0.5%: a 16 x 0.8 KB page
  **0.070 -> 0.032 ms (2.2x)**, while the same page at `threads=1` stayed flat at 0.028 ms,
  which is what says the saving is scheduling and not parsing. The 16 x 767 KiB batch is
  unaffected (-0.5%).

- **`str` payloads are borrowed instead of copied** (#226). `parse_email`,
  `parse_email_tree` and every `str` slot of `parse_many` duplicated the whole
  message into a fresh `Vec<u8>` before parsing. `bytes` stopped being copied in
  #96; `str` was left behind on the premise that the limited API has no UTF-8
  buffer to borrow, which is not true for the ABI this crate builds -- `abi3-py311`
  sets the `Py_3_10` cfg, under which `PyString::to_str` is the zero-copy
  `PyUnicode_AsUTF8AndSize` the code already called one line before the copy.
  `Payload` now holds a `PyBackedStr` for `str` exactly as it holds a
  `PyBackedBytes` for `bytes`, and has no owned variant left. No API change: `str`
  and `bytes` still produce identical results, non-text input still raises
  `TypeError`, and so does a `str` holding a lone surrogate (now pinned by a test
  that also passes against 0.9.0). Measured on an Apple M4 (10 vCPU), 5 interleaved
  rounds, controls within 1.6%: the new `mode="metadata"` benchmark fed a `str`
  **0.038 -> 0.030 ms (-21%)**, which is the `bytes` path's own 0.030 ms to three
  decimals -- the marshalling cost is gone rather than reduced -- and the str-fed
  gate benchmark `parse_message` **0.243 -> 0.232 ms (-4.5%)**, `parse_message_strict`
  0.240 -> 0.230 ms. Every bytes-fed benchmark stayed within 0.9%, so this is not a
  code-layout effect (#204).

- **Full-mode attachments and MIME-tree leaves stop copying on every read** (#227).
  Reading `mail.attachments` deep-cloned every attachment -- `#[pyo3(get)]` on a
  `Vec<pyclass>` field goes through PyO3's clone path, so each read `memcpy`'d every
  decoded payload and handed back fresh objects -- and each `.content` read then built
  another `bytes`. Two full copies per attachment, on the library's own documented
  `by_cid` idiom. `PyMail.attachments` is now `Vec<Py<PyAttachment>>` behind a
  `clone_ref` getter, and `PyAttachment.content`/`PyMimePart.content` publish one
  `bytes` through a `OnceLock` -- the shape lazy mode has had since #97. So
  `mail.attachments[0] is mail.attachments[0]` and `a.content is a.content` now hold in
  every mode, including under racing first reads from several threads. Nothing else
  moves: attribute sets, types, decode timing and where `DecodeError` raises are all
  unchanged. A caller holding an attachment and its bytes now keeps two copies of the
  payload alive rather than three. Measured on an Apple M4 (10 vCPU), 10 interleaved
  rounds, controls within 0.9%: the new `attachment_reread` benchmark -- one
  `attachments` read plus one `.content` read per attachment, parse excluded --
  **12.83 -> 0.083 us, ~156x**, and `full_read` (parse + read, decode-dominated)
  **0.250 -> 0.243 ms (-2.9%)**. Every other benchmark stayed inside the noise floor.

- **Base64 bodies decode with SIMD** (#228). `decode_base64` in the vendored mailparse
  stripped whitespace and then ran `data_encoding`'s scalar loop -- four table lookups per
  three bytes, at scalar peak on every CPU tried, and **53% of a full parse** after the
  strip was fixed in #214/#218. It now decodes with `base64_simd::STANDARD` (AVX2/SSE4.1
  with runtime detection on x86-64, NEON on aarch64) and keeps `data_encoding` as the
  arbiter of everything the SIMD decoder turns down. That fallback is what makes this a
  pure speed change: `STANDARD` accepts a strict subset on a whitespace-free buffer, so
  every message that decoded before still decodes, to the same bytes, and every rejection
  still carries `data_encoding`'s own `DecodeError { position, kind }` and reaches Python
  as the same `DecodeError` text. The two lenient cases this library has always
  accepted -- `=` mid-stream and non-zero trailing bits -- take the fallback and are pinned
  by tests. A differential test over a built corpus and a new `base64_agreement` fuzz
  target (3.6 M executions clean) hold the agreement up. Measured on an Apple M4 (10 vCPU),
  two independent interleaved A/Bs pooled to 8 rounds per side, pure-Python controls within
  1.1%: full `parse_email` **0.254 -> 0.168 ms (-34%)**, `parse_email_tree` 0.246 -> 0.159 ms,
  `full_read` 0.261 -> 0.172 ms, `parse_many` (8 x 767 KiB) **1.989 -> 1.318 ms (-34%)**.
  The CI gate's EPYC agrees, interleaved against the merge base on the same runner:
  `parse_email` **0.536 -> 0.316 ms (-41%)**, `parse_email_tree` 0.540 -> 0.320 ms,
  `full_read` 0.573 -> 0.354 ms, `parse_many` **4.382 -> 2.583 ms (-41%)**, controls within
  1.1%. `mode="metadata"` and untouched lazy parses never call this function and are flat
  on both (x86: +0.9% and -1.3%, inside the noise floor).

- **Fuzzing: the initial 24 CPU-hour campaign ran, found nothing, and its corpus now
  seeds the deep run** (#102). Two targets, 8,640 s x 5 workers each on an Apple M4:
  `parse_email` 12.1 M executions to 5,047 edges, `parse_agreement` 5.5 M to 5,502,
  zero crashes, timeouts or OOMs. The minimised corpora (129 MB raw, 8.3 MB compressed)
  ship as `fuzz-corpus-min.tar.gz` on a `fuzz-corpus-*` release rather than in the
  tree, and `deep-fuzz.yml` seeds from that asset when its cache is cold (lineage moved
  to `v3` so the next run does). `publish.yml` is guarded to `v*` tags so a corpus
  release does not build wheels.

## [0.9.0] - 2026-08-28

Measured against the published 0.8.0 wheel on an Apple M4 (interleaved, 3 rounds, controls
within 3.8%): full parse **1.079 -> 0.225 ms**, `parse_email_tree` 1.071 -> 0.216 ms,
`parse_many` (8 x 767 KiB) 8.852 -> 1.835 ms, `mode="metadata"` **0.365 -> 0.030 ms**.

The version bump itself moved the decoding paths **+3-7%** against the last master commit on
the M4 (two interleaved A/Bs, metadata paths within 0.7%) -- the code-layout effect #204
describes, now measured at release time rather than discovered later. The two loops that made
it 0-96% are fixed; what remains is the base64 decode proper.

### Changed

- **The base64 whitespace strip in the vendored mailparse is now dependency-free**, and
  faster: `vendor/mailparse/src/bytescan.rs` skips any word with no byte below `0x21`, walks
  the exact `hasless` mask of one that might, and copies runs in one piece -- plain `std`,
  no `unsafe`. One pass where the `memchr` version made two searches per run: full parse
  0.254 -> 0.228 ms and `parse_many` 2.09 -> 1.83 ms on an Apple M4, -2.5 to -2.9% on the
  CI gate's EPYC 7763. The MIME boundary search keeps `memchr`: dependency-free versions of
  it measured +13-14% (four words per branch) and +9-11% (eight) on the metadata paths on
  x86, so it stays. Context: upstream declined the `memchr` change as an added dependency
  (staktrace/mailparse#142); a dependency-free version of both loops is offered instead
  (staktrace/mailparse#143), and `vendor/mailparse/PATCH.md` records what switching to it
  would cost if a release carries it.
- **Rust toolchain pin moved from 1.97.1 to 1.98.0** (#120). The pin was a holding
  action against a measured 15-30% slowdown under 1.98.0; the cause turned out to be
  the two mailparse loops above -- the compiler had not changed their instructions,
  only their addresses -- and with those replaced the toolchain A/B on the CPU that
  showed the worst of it reads 1.98.0 within +/-0.5% of 1.97.1 on every parse path.
  Published wheels are built with 1.98.0 from the next release. The pin stays a pin
  rather than floating to `stable`, because the benchmark gate builds both sides
  with the same compiler and so cannot see a compiler regression; bumping it is a
  measured change (`toolchain-ab.yml`), described in `rust-toolchain.toml`.
- **Base64 whitespace is stripped with `memchr`** instead of a test-and-push per byte,
  the second change in the patched `vendor/mailparse`. With the
  boundary search fixed, that filter was **77.7%** of a full parse of the 767 KiB fixture
  -- more than the base64 decode itself -- and the toolchain A/B showed it carried the
  rest of the rustc placement sensitivity: decoding paths still moved +22% on one CPU
  and +5% on another while the metadata paths had gone flat. The bytes removed are
  unchanged (exactly `u8::is_ascii_whitespace`), checked against the original filter
  over every byte value. Gate on the PR, median of three interleaved rounds on the CI
  runner (AMD EPYC 7763), against the master with the boundary fix: full parse
  **1.432 -> 0.575 ms**, `parse_email_tree` 1.419 -> 0.556 ms, `parse_many` (8 x 767 KiB)
  11.29 -> 4.48 ms; metadata paths unchanged, as they decode nothing; the cross-library
  table now reads 25x over mail-parser and 34x over the stdlib. An Apple M4 measured
  0.830 -> 0.281 ms for the same pair, and 1.094 -> 0.281 ms against the master before
  either change.
- **The MIME boundary search is now `memchr::memmem`** instead of a byte-by-byte scan,
  through a patched copy of `mailparse` 0.16.1 in `vendor/mailparse`. The copy is
  permanent: the change was proposed upstream and declined (staktrace/mailparse#142 --
  no new external dependencies), so `vendor/mailparse/PATCH.md` describes how to merge
  future mailparse releases into it rather than how to remove it. That one loop was
  **96.5%** of a metadata-mode parse, and it was the codegen cliff #120 and #204 were
  circling: the same 88 instructions ran at half speed on x86-64 depending on where the
  linker placed them, so a rustc minor version or a version-string bump could move the
  metadata path by up to 96% with no change to the code being run. Gate on the PR,
  median of three interleaved rounds on the CI runner (AMD EPYC 9V45), against master:
  full parse **1.053 -> 0.604 ms**, `mode="metadata"` **0.362 -> 0.030 ms**,
  `mode="lazy"` untouched 0.382 -> 0.048 ms, `parse_email_tree` 1.048 -> 0.595 ms,
  `parse_many` (8 x 767 KiB) 8.53 -> 4.82 ms, `parse_many(mode="metadata")`
  2.96 -> 0.25 ms; the cross-library table went from 6.4x to 11.8x over mail-parser.
  An Apple M4 measured -32% / -92% for the same pair -- smaller, because it never
  had the slow layout to lose. Every mode faster, no output changes (803 tests,
  both fuzz targets' corpora). `memchr` 2.8.3 is the only
  new crate in the lockfile.
- **`fast-mail-parser-ng` 0.7.1** was published as that distribution's final
  release, ahead of archiving it on PyPI. It is 0.7.0's code with a deprecation
  notice for a description — PyPI's guidance is to leave a retired name pointing
  somewhere rather than going quiet, and an archived project's page is the last
  thing a reader of a stale pin will see. Building it from `v0.7.0` rather than
  from master was deliberate: shipping 0.8.x features under the retired name would
  have created a second source of truth and removed the reason to migrate. The
  four versions under that name remain installable.

### Added

- **`mode=` on `parse_many` and `parse_email_tree`** (#202), honouring the same
  `"full"` / `"lazy"` / `"metadata"` values `parse_email` honours. The mode axis
  shipped on one entry point of three; these are the two where it is worth the
  most, and until now a caller had to choose between a mode and the API.
  - **`parse_many(payloads, mode="metadata")`** is the mailbox sweep the mode was
    built for. The batch API removes per-message overhead (#96: ~12x at
    2000 × 0.8 KB) and metadata mode removes the decoding (#97: ~3x on an
    attachment-heavy message); they now compose. Median of three interleaved
    rounds on the CI runner, 767 KiB fixture, 8 messages, `threads=1`: full
    16.16 ms, metadata 4.68 ms — **3.5x**, the same ratio the single-message mode
    gets. On 2000 × 0.8 KB it is 4.11 ms against 4.67 ms, a 1.14x edge, because
    small messages are mostly headers and there is little decoding to skip.
  - **`parse_email_tree(payload, mode="lazy")`** is the forensics case: walk the
    structure of a large message and decode the one part you want. A full-mode
    tree decodes every leaf, which is the wrong bill for exactly what the tree is
    best at. `mode="metadata"` decodes nothing *and retains nothing*. On the same
    fixture and runner: full tree 2.02 ms, lazy with nothing read 0.61 ms,
    metadata 0.58 ms — **3.3x** and **3.5x**.
  - **Two new node types, `PyLazyMimePart` and `PyMimePartMetadata`**, rather than
    a wider `PyMimePart`. `PyMimePart.content` is `bytes | None` and its `None`
    *means* "this node is a `multipart/*` container"; a mode where it also meant
    "not decoded yet" would make a leaf indistinguishable from a container, which
    is the silent-wrong-answer shape #150 and metadata mode's absent `text_plain`
    were both about. A metadata node therefore has no `content` attribute at all
    and reports `encoded_size` instead — `None` in the same place, and with the
    same meaning, as `content`'s `None`. Making `PyMimePart.content` lazy would
    also have re-timed a shipped attribute and moved where it raises, which is
    what #104 batches into an API-v2 window; adding a type is not.
  - **Two rather than one**, because the two differ in more than presentation:
    lazy mode retains a copy of every leaf so it can decode one later, metadata
    mode retains none. On a tree that difference is the memory, which is the point
    of the sweep.
  - **The tree is the same tree in every mode.** Every per-node field except the
    body is asserted equal to full mode's across the whole fixture and RFC corpus,
    and on arbitrary input by the `parse_agreement` fuzz target, which gained the
    #202 derivations. A mode that quietly reshaped a message would be the exact
    failure that target exists to catch, and it has caught two already.
  - **`message/rfc822` is still decoded, in every mode.** Its body *is* the
    embedded message, so parsing it is what gives the node children — deferring it
    would defer the structure, which is the one thing a tree API has to deliver
    eagerly. The bytes are published rather than dropped and decoded again, so
    such a node has `is_decoded == True` from the start; and unlike
    `parse_email(mode="metadata")`, a deferred tree can raise `DecodeError` for
    such a part. Documented, and pinned as a test.
  - **The lazy tree extends the `raw_bytes` claim to a root.** Flat lazy mode
    defers only subparts; a single-part message's tree root *is* the leaf, so the
    retained slice is the whole payload. That the re-parse still reproduces full
    mode's bytes is asserted per fixture and on arbitrary input rather than
    assumed.
  - **Combinations that cannot mean anything are refused.** `strict=True` with
    `mode="metadata"` raises `ValueError` on `parse_many` with the same message it
    raises on `parse_email`, and before any parsing happens. `strict=True` with
    `mode="lazy"` is honoured per slot and means what it means everywhere else.
    The tree has no `strict` in any mode, because it has no warnings channel to be
    strict about.
  - `walk` accepts a node from any mode and yields nodes of the same type. Its
    implementation was already duck-typed; the stub now says so with a
    value-restricted `TypeVar`, which is non-breaking — walking a `PyMimePart`
    still yields `PyMimePart`.
  - **Measured, not assumed.** The CI benchmark gate's interleaved A/B against
    `origin/master`: worst treatment delta **+0.8%** against an 8.2% control noise
    floor, `parse_message` +0.5%, `parse_many` -0.2%, `parse_tree` +0.7%,
    `parse_metadata` -0.0% — no significant difference, and positive means the
    base was the slower side. The default paths are answered in the first lines of
    their entry points and are byte-for-byte what they were; every new binding
    function carries `#[inline(never)]`, the two error constructors are the
    existing `#[cold]` ones, and each new mode pair shares one
    `catch_panics` call site. #99, #100, #135 and #180 are four records of code
    that never executes costing this hot path 15-96%.
  - The batch scheduler is now shared rather than copied: `parse_many_as` takes the
    per-message parse as a `fn` pointer, and `parse_many` is a call to it. The
    dynamic cursor and the order restoration are where a bug would be subtle, so
    there is one of them and not three.

- **`parse_email(payload, mode="lazy")`** decodes the bodies as usual and defers
  each attachment: `PyLazyAttachment.content` decodes on first access and caches,
  returning a `PyLazyMail` (#97). This completes the issue: `mode="metadata"`
  shipped in 0.8.0, and the mode axis and the overload pattern it introduced were
  built for this to slot into.
  - **An attachment nobody reads is never decoded**, which is the whole point and
    is asserted rather than timed: `is_decoded` is a public attribute, so
    "reading one attachment leaves the others untouched" is a test rather than an
    argument. It also answers a real question for a caller holding an inventory --
    whether reading `content` is free or is about to cost a decode.
  - **Repeated access returns the same object.** The cache holds the `bytes`
    object, not the bytes, so `a.content is a.content` and the second read
    allocates nothing.
  - **Thread-safe without a lock.** The GIL is released for the decode, so
    concurrent readers overlap; the cache is a `OnceLock` published once, so every
    caller -- winner or loser of a race -- returns the object the cell holds. Two
    threads arriving together can both decode, which is bounded duplicate work
    rather than a correctness problem: `OnceLock` has no fallible init on stable,
    and the alternatives were panicking on a broken transfer encoding or caching
    the failure. A failed decode is not cached at all.
  - The free-threading audit's invariant is intact. That audit's note named a
    decode cache as the obvious way to break it; the cache is in the binding
    layer, on the Python object that owns the retained bytes, and the parsing
    core's contribution is a pure function of `&[u8]`. Nothing in the core is
    shared or mutable.
  - **`encoded_size` on `PyLazyAttachment`**, the same value and name as in
    metadata mode. Without it the mode would not work: choosing which attachment
    to decode must not require decoding any of them.
  - **A `DecodeError` moves from the parse to the attribute.** A part whose
    transfer encoding is broken fails `parse_email` in full mode and fails on
    `content` here, so a message with one broken attachment stays readable. Pinned
    as a test rather than left to be discovered, like metadata mode's inability to
    report one at all.
  - `strict=True` works with this mode and means exactly what it means in full
    mode: lazy mode decodes every body and finds every repair full mode finds --
    the one attachment-level repair the parse can report is found by scanning the
    *encoded* bytes -- so `warnings` is the same list. Asserted per fixture across
    the corpus, and on arbitrary input by the fuzz target.
  - **The trade, stated plainly: memory for decoding.** The encoded bytes of every
    attachment are retained, and base64 is ~1.33x the size of what it encodes, so
    a retained part costs more than the decoded bytes it avoids producing. On the
    attachment-heavy fixture, median of three interleaved rounds on the CI
    runner: metadata 0.52 ms, lazy with nothing read 0.56 ms, full 2.00 ms, lazy
    with every attachment read 2.04 ms. Deferring saves ~70% when you were not
    going to decode everything and costs ~2% when you were. **Use the default
    mode if you are going to read every attachment.**
  - A new type rather than a lazier `PyAttachment.content`. Widening or
    re-timing an existing attribute is a breaking change, and #104 batches those
    into one API-v2 window; the default path is untouched, and `PyMail` and
    `Attachment` are byte-identical.
  - Implemented by retaining each part exactly as it sits in the message --
    mailparse's `raw_bytes`, which for a subpart is precisely that part's headers,
    separator and still-encoded body -- and re-parsing that copy on access. Which
    reproduces the full parse's `content` only if that claim about mailparse's
    slicing is true, so it is tested as one: equal for every attachment across the
    fixture and RFC corpus, and asserted on arbitrary input by the
    `parse_agreement` fuzz target, which now also compares the envelope, the
    bodies and the whole warning list between the two modes.
  - The default path is unchanged, and deliberately so at the source level: the
    new mode is answered inside the `#[inline(never)]` helper that already handles
    metadata mode, `parse_email`'s body is byte-for-byte what it was, and strict
    mode gained a branch inside its existing `catch_panics` rather than a second
    call site. #99 and #100 both measured what an extra instantiation of that
    generic can cost the hot path while never executing.

## [0.8.0] - 2026-08-27

### Changed

- **The distribution is published as `fast-mail-parser` again.** `pip install
  fast-mail-parser`. Releases 0.6.0–0.7.0 went out as `fast-mail-parser-ng`
  because the original name belonged to an account this project no longer
  controlled and was frozen at 0.2.5 from June 2022; ownership has since been
  transferred directly, so the PEP 541 request was withdrawn.
  - **The import path is unchanged**, as it has been throughout:
    `from fast_mail_parser import parse_email`.
  - On `fast-mail-parser-ng`? Change the name in your requirements file and
    nothing else. That project is archived on PyPI -- read-only, no further
    releases -- and the three versions published under it stay installable, so
    nothing pinning them breaks.

### Added

- **`PyMail.warnings`** reports the lossy repairs a parse performed, and
  **`parse_email(payload, strict=True)`** raises them instead (#100). Additive:
  the default path is unchanged, and no existing attribute changed name or type.
  - The parser has always been best-effort in the middle ground between "valid"
    and "invalid" — an unresolvable charset label is decoded as us-ascii, an
    unparseable address header yields no mailboxes, an unreadable `Date` leaves
    `date_parsed` at `None`, an unterminated header block is resynced. Every one
    of those returned a result and said nothing. The new `list[ParseWarning]`
    says which, where, and what was lost.
  - **The empty list is the contract.** `warnings == []` means nothing was
    patched up, so a pipeline can route everything else to quarantine instead of
    classifying a message on content that was silently mended. Asserted over the
    whole fixture and RFC corpus, so good mail cannot start warning by accident.
  - Kinds emitted today: `charset-fallback`, `address-unparseable`,
    `date-unparseable`, `transfer-decode-lossy`, and `unterminated-header-block`
    — the last of which is
    #150's repair becoming observable. That message's header block is never
    closed, so the payload is resynced before `mailparse` reads it; the repair is
    what saved the body, and this is what says the repair happened.
    `ParseWarning.part_path` locates the affected value in the result
    (`"text_plain[0]"`), or is `""` for a message-level warning.
  - `part_path` is a locator into the returned `PyMail` rather than MIME tree
    coordinates, which the issue proposed. The flat parse discards the structure
    a coordinate would name, and every way of reconstructing it charges *every*
    message for bookkeeping almost none needs — in the same traversal where #135
    measured a 30% regression from widening what it carries. Tree coordinates
    belong on `parse_email_tree`, whose walk already has them.
  - `strict=True` maps each kind onto the existing hierarchy —
    `address-unparseable` to `HeaderParseError`, `unterminated-header-block` to
    `MimeStructureError`, the other two to `DecodeError` — so it adds no
    exception types and `except ParseError` still catches everything. It adds
    rejections rather than reclassifying: a message that
    parses cleanly parses identically either way, and one that fails outright
    fails with the same type. `parse_many(payloads, strict=True)` applies it per
    slot.
  - `strict=True` requires `mode="full"` and raises `ValueError` with
    `mode="metadata"`, which also has no `warnings` attribute. Metadata mode
    never reads a body, so the strongest thing it could say is "nothing in the
    *headers* was repaired" — and the same attribute name with a weaker
    guarantee would break the one property the channel exists to provide. Same
    reasoning that leaves `text_plain` absent from that mode rather than empty.
  - Free when there is nothing to report, which is the case that matters. The
    collector is a `Vec` that does not allocate until pushed; every warning site
    is a branch well-formed mail does not take; and each one's `format!` lives in
    a `#[cold]`/`#[inline(never)]` helper, because a `format!` inlined into the
    per-part loop is the exact shape that cost 30% in #135. Strict mode is a
    check *after* the parse rather than a flag threaded through it, so the core
    never learns about it.
  - Not reported, and deliberately: a broken quoted-printable body is repaired
    silently by mailparse's decoder rather than reported by it — it decodes
    quoted-printable in robust mode — so observing it would mean decoding
    twice. Recorded here rather than left as an omission a reader would mistake
    for completeness.

- A second fuzz target, `parse_agreement`, checks metadata mode and the tree API
  against the flat parse on arbitrary input rather than only for absence of
  panics: the envelope and the attachment inventory must agree between modes, and
  every attachment the flat parse reports must appear in the tree (#102).
  - It found one on its first run: `mode="metadata"` was not repairing a missing
    header/body separator, so it disagreed with the full parse about such a
    message's headers. Fixed here. The corpus test for that mode had been
    excluding the only fixture with the defect, which is why nothing else caught
    it.
- **`parse_email(payload, mode="metadata")`** reads the headers and the attachment
  inventory without transfer-decoding anything, returning a `PyMailMetadata`
  (#97). On an attachment-heavy message that is most of the work skipped.
  - The mode picks the return type statically through overloads, so callers of the
    default path see no change: `PyAttachment.content` stays `bytes` rather than
    widening to `bytes | None`.
  - Attachments come back as `PyAttachmentMetadata` with `encoded_size` -- the
    bytes the part occupies in the message, before decoding. Named for what it is:
    the decoded size cannot be known without the decode this mode exists to skip.
    It is **not** an upper bound on the decoded size -- quoted-printable emits a
    line break as CRLF, so a body of bare LFs decodes larger than it was encoded.
    Decoding cannot more than double a part.
  - No `text_plain`/`text_html`, and absent rather than empty: an empty list
    cannot be told apart from "no text part", so a sweep counting bodyless
    messages would count all of them.
  - It cannot report `DecodeError`, because it never decodes. Header errors are
    reported in both modes.
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
  - The repair is now also reported: it records an `unterminated-header-block`
    entry on `PyMail.warnings` (#100), so a consumer can tell that this message
    needed patching rather than only that it parsed.
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

[Unreleased]: https://github.com/namecheap/fast_mail_parser/compare/v0.9.0...HEAD
[0.9.0]: https://github.com/namecheap/fast_mail_parser/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/namecheap/fast_mail_parser/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/namecheap/fast_mail_parser/compare/v0.6.1...v0.7.0
[0.6.1]: https://github.com/namecheap/fast_mail_parser/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/namecheap/fast_mail_parser/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/namecheap/fast_mail_parser/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/namecheap/fast_mail_parser/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/namecheap/fast_mail_parser/releases/tag/v0.3.0
