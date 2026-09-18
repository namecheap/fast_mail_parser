# Contributing to fast_mail_parser

Thanks for your interest in contributing! `fast_mail_parser` is a Python
library for parsing `.eml` files, implemented in Rust and exposed to Python via
[PyO3](https://github.com/PyO3/pyo3) and built with
[maturin](https://www.maturin.rs/). This guide covers how to build the project
from source, run the tests, and what the CI expects from a pull request.

For bug reports and feature requests, please
[open an issue](https://github.com/namecheap/fast_mail_parser/issues) first to
discuss what you would like to change.

## Prerequisites

- **Rust** — the toolchain is pinned in
  [`rust-toolchain.toml`](rust-toolchain.toml) to **1.98.1**; if you use
  `rustup`, the correct version is selected automatically in this directory.
  The pin is deliberate: the benchmark gate builds both sides with the same
  compiler, so a compiler change is the one regression it cannot see — bump the
  pin with the `toolchain-ab.yml` measurement, as the comment in that file
  describes. The MSRV is
  a separate, lower bound (**1.83**, declared as `rust-version` in
  [`Cargo.toml`](Cargo.toml)). Some CI checks (`cargo audit`) run on **stable**
  rather than the pinned version.
- **Python** — **3.11–3.14** (`requires-python = ">= 3.11"` in
  [`pyproject.toml`](pyproject.toml)). The CI test matrix covers 3.11, 3.12,
  3.13, and 3.14.
- **[maturin](https://www.maturin.rs/)** — the build backend (declared in
  `pyproject.toml` as `maturin>=1.0,<2.0`). It compiles the Rust extension and
  installs it into your environment.
- A C toolchain / linker, as required to compile the native extension.

## Build from source

Work inside a virtual environment so the compiled extension is installed in
isolation:

```bash
python -m venv .venv
source .venv/bin/activate        # Windows: .venv\Scripts\activate

pip install maturin
```

Then build and install the extension into the active environment:

```bash
# Debug build (fast to compile, slower at runtime) — recommended for development.
maturin develop

# Release build — required when benchmarking or measuring performance.
maturin develop --release
```

After `maturin develop`, the module is importable:

```python
from fast_mail_parser import parse_email, ParseError
```

### Makefile targets

The [`Makefile`](Makefile) wraps the common workflows (these install the
package via `pip install .`, which uses the maturin build backend):

| Target              | Command                                      | Purpose                                        |
| ------------------- | -------------------------------------------- | ---------------------------------------------- |
| `make install`      | `pip install .`                              | Build and install the package.                 |
| `make install-test` | `pip install ".[test]"`                      | Install the package plus test dependencies.    |
| `make test`         | `pytest -v --ignore tests/benchmark tests`   | Run the test suite (excludes the benchmark).   |
| `make benchmark`    | `pytest -v tests/benchmark`                  | Run the performance benchmark only.            |

For day-to-day development, `maturin develop` is the quickest way to rebuild
after editing the Rust source; `make install` / `make install-test` mirror what
CI does.

## Running tests

Install the package together with its test dependencies, then run the suite:

```bash
make install
make install-test
make test
```

`make test` runs `pytest` over `tests/`, excluding `tests/benchmark` (which is
a performance benchmark, not a correctness test — see below).

The test suite includes:

- [`tests/test_contract.py`](tests/test_contract.py) — freezes the **public API
  contract**: the exported names (`parse_email`, `PyMail`, `PyAttachment`,
  `PyAddress`, and the `ParseError` hierarchy — `HeaderParseError`,
  `MimeStructureError`, `DecodeError`), the attribute set and types of each, the
  input types `parse_email` accepts (`str` and `bytes`), and the errors it
  raises. The attribute and export sets are *frozen* there, so any addition has
  to move them deliberately — a failure here means a consumer-visible change.
- [`tests/test_rfc_corpus.py`](tests/test_rfc_corpus.py) — characterization
  tests over an **RFC-feature `.eml` corpus** in `tests/data/rfc/`, locking the
  parser's actual output per email/MIME RFC feature (multipart, base64,
  quoted-printable, RFC 2047/2231/6532, folded headers, etc.).
- [`tests/test_stdlib_parity.py`](tests/test_stdlib_parity.py) — a
  **differential suite against the stdlib `email` module**. Both parsers run over
  the whole corpus and are compared on nine dimensions; a mismatch fails unless
  it is a declared divergence in `DIVERGENCES`, and a declared divergence that
  stops occurring fails too. It is the regression oracle for the correctness
  work, and every row of [`docs/compatibility.md`](docs/compatibility.md) is a
  key in that table.
- [`tests/test_docs_snippets.py`](tests/test_docs_snippets.py) — executes every
  Python snippet in [`docs/migrating.md`](docs/migrating.md) in document order
  against the built wheel, so the migration guide cannot drift from the API.
- [`tests/test_contents.py`](tests/test_contents.py),
  [`tests/test_attachments.py`](tests/test_attachments.py),
  [`tests/test_headers.py`](tests/test_headers.py),
  [`tests/test_multivalue_headers.py`](tests/test_multivalue_headers.py),
  [`tests/test_addresses.py`](tests/test_addresses.py),
  [`tests/test_date_parsed.py`](tests/test_date_parsed.py),
  [`tests/test_error_taxonomy.py`](tests/test_error_taxonomy.py),
  [`tests/test_empty_fields.py`](tests/test_empty_fields.py) — focused tests on
  body text, attachments, repeated headers, typed addresses, date parsing, the
  error hierarchy, and empty-field handling.

### Regenerating the RFC corpus

The fixtures under `tests/data/rfc/` are generated deterministically (fixed
dates, message-ids, and MIME boundaries, so output is byte-identical across
runs). If you intentionally change a fixture or add a new RFC feature, regenerate
them with:

```bash
python tests/generate_rfc_corpus.py
```

then commit the regenerated `tests/data/rfc/*.eml` files. Note that
`tests/test_rfc_corpus.py` asserts the on-disk corpus and the in-test `CASES`
table stay in sync, so add or update the corresponding `CASES` entry when you
add a fixture.

### Benchmark

The benchmark suite serves two different purposes, and the tests are kept
separate because of it.

Two benchmarks are the **CI gate** and are deliberately stable rather than fair:
the `mail-parser` baseline measures `MailParser.from_string`, which never calls
`.parse()`. Three `*___full_read` benchmarks are the **published comparison**,
asking each library for the same result.

Build with `--release` first, then:

```bash
maturin develop --release
make benchmark        # run the benchmarks
make bench-table      # render the comparison table published in Readme.md
```

`make bench-table` prints its own methodology line (corpus, CPython version,
machine, library versions). Paste both together — the ratios move with the CPU,
so a number without its machine is not a measurement.

## Fuzzing

The parser exists to read attacker-controlled bytes, so there is a fuzz harness
in `fuzz/`. It requires nightly:

```bash
cargo install cargo-fuzz --locked
mkdir -p fuzz/corpus/parse_email
cp tests/data/*.eml tests/data/rfc/*.eml fuzz/corpus/parse_email/
RUSTUP_TOOLCHAIN=nightly cargo fuzz run parse_email
```

`RUSTUP_TOOLCHAIN` is needed because `rust-toolchain.toml` pins a stable version;
the env var is the only thing that overrides a toolchain file.

Three invariants are asserted on arbitrary input:

- **No panic** — still the one that matters most, though not because a panic
  takes down the host process: it never did. PyO3 catches panics at the boundary,
  and since #162 the binding converts them to `ParseError`. What a panic costs is
  a message reported as unparseable instead of parsed, so finding one here is
  finding it before a mail pipeline does.
- **Determinism** — the same bytes parse the same twice, compared over every
  field, headers **in order**. The harness's first run failed here and it was a
  real bug (#157), not a flawed check.
- **Bounded output** — decoded size stays within a generous multiple of the
  input, so nothing can amplify its way through memory.

Two things about the harness are deliberate and worth knowing before changing it:

- It is a **standalone crate, not a workspace member.** The root crate is
  `crate-type = ["cdylib"]` and its lib root is the PyO3 binding layer, so it can
  neither be linked as a Rust dependency nor built without Python symbols.
- It therefore **depends on `crates/fast_mail_parser_core`**, the same crate the
  extension depends on. That works only because the core has no PyO3 dependency.
  If it ever gains one, this harness stops building — which is the right alarm,
  since the split is what the core's docs promise, and `cargo tree -p
  fast_mail_parser_core | grep pyo3` is how to check it deliberately.

Until #236 the targets `#[path]`-included `src/mail_parser.rs` instead, and
`fuzz/Cargo.toml` duplicated the `charset` and `mailparse` versions from the root
manifest — two copies of one source compiled under different cfg, with nothing to
say when they drifted. The core being a real crate removes both problems: the
version requirements live in its manifest alone, and the harness links what
ships.

CI runs a 60-second deterministic pass on every PR, seeded so it stays
reproducible; a crasher is uploaded as an artifact.

There are two targets. `parse_email` fuzzes the flat parse for the three
invariants above. `parse_agreement` fuzzes the two APIs added since — metadata
mode (#97) and the tree (#99) — and asserts **agreement** rather than only absence
of panics, because a re-derivation of the same message is most likely to be wrong
by disagreeing with the original:

- metadata mode must succeed wherever the full parse does, since it does strictly
  less work (the converse is *not* asserted: a broken transfer encoding fails the
  full parse and passes metadata, which is a documented difference)
- subject, date and the whole header map must be identical between the two modes
- the attachment inventory must match, field by field, except content and size
- `encoded_size` is never below the decoded size, since no transfer encoding
  shrinks its input
- the tree parses deterministically, and every attachment the flat parse reports
  appears as some leaf's content

A **weekly deep run** (`.github/workflows/deep-fuzz.yml`, Mondays) fuzzes for 30
minutes per target with no fixed seed and, importantly, **caches the corpus
between runs**,
so coverage compounds instead of restarting from the fixtures every week — that
accumulation is most of what makes a scheduled run worth more than the PR pass.

**The initial 24 CPU-hours** the fuzzing issue (#102) asked for ran on 2026-08-28,
locally: two targets, five forked workers each, 8,640 s per target, ASan, on an
Apple M4. `parse_email` executed 12.1 M inputs and reached 5,047 edges,
`parse_agreement` 5.5 M and 5,502; neither found a crash, timeout or OOM. The
minimised corpora (3,135 and 4,234 inputs) are too large to commit -- 129 MB raw,
8.3 MB compressed -- so they live as `fuzz-corpus-min.tar.gz` on the newest
`fuzz-corpus-*` GitHub release, and the deep run seeds from that asset whenever
its cache is cold. To start from it locally:

```bash
gh release download "$(gh release list --json tagName --jq '[.[].tagName|select(startswith("fuzz-corpus-"))]|sort|last')" \
  --pattern fuzz-corpus-min.tar.gz --dir /tmp
tar -xzf /tmp/fuzz-corpus-min.tar.gz -C fuzz/corpus/parse_email --strip-components=2 corpus-min/parse_email
RUSTUP_TOOLCHAIN=nightly cargo fuzz run parse_email -- -fork=5 -ignore_crashes=1 -max_total_time=3600 -max_len=65536 -timeout=10
```

Fork mode keeps going after a crash and leaves each one in `fuzz/artifacts/`; a
single-process run stops at the first. Refreshing the asset is `-merge=1` from the
accumulated corpus into an empty directory, then a new `fuzz-corpus-<date>` release.

Generated inputs are capped at 64 KiB (`-max_len`). Uncapped, libFuzzer takes its
limit from the largest seed — and the seed corpus contains a 0.75 MiB message — so
it spent its time on ~59 KiB inputs at ~158 executions per second. Bugs per minute
is the thing being bought here, and oversized-input behaviour is covered directly
by `tests/test_dos_limits.py` rather than by hoping the fuzzer stumbles into it.

A crasher there is minimised, uploaded, and reported onto a single pinned issue
labelled `fuzz-crash`: one issue that gets comments, not a new issue per week,
because a crasher stays in the corpus and keeps being found until it is fixed.
Anything found should be minimised and committed as a regression fixture under
`tests/`, so the finding stays found.

To rehearse that reporting path without waiting for a real crash, dispatch the
workflow with `inject_canary`. That sets `FMP_FUZZ_CANARY` in the environment,
which arms a deliberate panic in the target, and the resulting issue says so —
close it afterwards.

The canary is armed through the environment rather than by a magic input, and the
first attempt got that wrong in two ways worth knowing before changing it.
libFuzzer intercepts `memcmp` and **learns** the operands input is compared
against, so a literal comparison is an instruction to the fuzzer rather than a
secret from it — it found a 32-byte magic string within seconds. And the bytes had
to be planted in the corpus, which is cached, so every drill left its staged crash
behind for later runs to trip over. Both produced false crasher issues.

A drill **passes green**: with `inject_canary` set, the job succeeds when the
canary crashed and was reported, and fails when it did not, because a canary that
goes undetected means the alarm is broken. A normal run fails on any crash, as
you would expect.

The deep run is a matrix and that verdict is decided **per job**, so every target
needs its own canary. A target without one fails a drill by reporting that the
alarm is broken — true of that job, misleading about the alarm. Adding a target
means adding the four-line `canary_armed` check to it.

## Performance

There is a benchmark gate on every pull request. It builds this revision **and**
its base, then alternates measurement rounds between them and compares the
medians, failing on more than a 7% regression. The pure-Python benchmarks ride
along as a noise floor: they cannot be affected by how the Rust extension was
built, so a result is only believed once it clears them.

The base it compares against is the base branch **as it is now**, not the commit
recorded when the pull request was opened. A pull request is checked out as its
merge ref, so a stale base would attribute everything master did in the meantime
to the branch -- which is how #198 came to be reported at +15.2% for a build
byte-identical to one measuring +0.2%.

The interleaving is not ceremony. Pairing each build with its own measurement --
which this gate used to do -- leaves a ~16% artifact from the build cycle itself,
more than twice the threshold it is judging against. A toolchain comparison built
that way reported -0.2% on what an interleaved measurement then showed to be
+15.7% (#120).

Two things to know before changing anything in `src/`:

**This crate is unusually sensitive to codegen.** A rustc minor version alone
moved the parse path 15-96% (#120), which is why `rust-toolchain.toml` pins one.
That case has since been traced to two loops and fixed -- see below -- and the
pin has moved on past it, but the lesson stands: do not assume a change is
free because it looks free.

**Cold code can slow the hot path.** `catch_panics` is generic, so every entry
point instantiates it, and adding a third instantiation stopped the one wrapping
`parse_email` from being inlined -- costing 24% on large messages while the new
code was never executed (#99). It carries `#[inline(always)]` for that reason and
the comment says so; the tree API's binding functions carry `#[inline(never)]` for
the mirror reason. If the gate reports a regression from a change that "cannot
possibly" affect the hot path, this is the first thing to suspect, and bisecting
into throwaway branches is how it was found rather than guessed.

Both `lto = "fat"` and `codegen-units = 1` are already set, so a codegen-unit
boundary is never the explanation -- worth knowing, since it is the natural first
guess.

**The cliff had two causes, both byte-at-a-time loops in mailparse, both fixed.**
Sampling a metadata-mode parse of the 767 KiB fixture put **96.5%** of the time in
one function: mailparse's `find_from_u8`, the scan `parse_mail` runs for every MIME
boundary.
Its x86-64 instruction stream was byte-identical under rustc 1.97.1 and 1.98.0 --
88 instructions, only label hashes differed -- yet the runners measured +96% on
the metadata path. The compiler had not made the loop slower; it had *moved* it.
The crate hash changes with the rustc version, and with the package version
(#204), which changes symbol names, link order, and so the loop's address; on the
runners' Zen CPUs a scalar loop straddling the wrong 64-byte boundary falls out
of the micro-op cache and runs at half speed. An Apple M4 measured the same two
builds at +/-0.2%.

The fix replaces that scan with `memchr::memmem` -- vectorised, and laid out
independently of this crate -- via the patched copy in `vendor/mailparse`
(upstream declined the change as an added dependency and is offered a
dependency-free version instead; `vendor/mailparse/PATCH.md` has the reasoning,
the sync procedure and what switching would cost). Metadata mode went
0.365 -> 0.030 ms and the full parse 1.10 -> 0.76 ms.

With that gone, sampling the *full* parse put **77.7%** of what remained in
`decode_base64`'s whitespace filter -- `iter().filter().cloned().collect()`, a
test and a push per byte -- and the toolchain A/B confirmed it carried the rest of
the placement sensitivity: decoding paths still moved +22% on one runner and +5%
on another while the metadata paths had gone flat. Same place, and here the fix
is dependency-free (`vendor/mailparse/src/bytescan.rs`): a word with no byte below
`0x21` is skipped whole, the mask of one that might says which bytes to check,
and the runs between whitespace are copied in one piece. The full parse went
0.83 -> 0.28 ms on top, and 0.25 -> 0.23 again when this one-pass version replaced
the `memchr` two-search one.

**What remains true.** The gate has a false-positive mode its noise floor cannot
see: the controls are pure Python and do not care how the extension was laid out,
so they stay flat while every treatment benchmark moves together, tightly and
repeatably. The two dominant instances are gone, but the decode loops and charset conversion
are also placed by the linker, and the effect did not go with them. On
2026-09-17 three consecutive PRs failed the gate on `parse_qp_message` -- +9.2%,
+7.3% and +19.0% -- for changes that cannot reach quoted-printable decoding at
all; one of them touched only the `parse_many` thread scheduler, and on every one
of those runs the same message in `mode="metadata"`, which runs the headers and
skips the decode, got *faster*. Two `#[inline(never)]` attempts moved the number
to +7.5% and then to +12.9%. So: **re-run a large failure before acting on it**,
and do not patch placement by guesswork -- three binaries was enough to show it
does not converge.

### Measuring the core directly

The pytest benchmarks are the oracle for what the wheel costs a user, and a poor
instrument for asking *where* that cost is: every number carries the FFI
crossing, the GIL and the construction of Python objects. `bench/` measures the
core in Rust instead.

```sh
cd bench
cargo run --release -- ../tests/data/large_message.eml full
cargo run --release -- ../tests/data/large_message.eml strip --rounds 200
cargo run --release -- ../tests/data/large_message.eml b64-simd
cargo run --release -- ../tests/data/large_message.eml b64-scalar
```

`strip` and `b64-*` isolate the two halves of the base64 path, which nothing else
can. `b64-simd` against `b64-scalar` is what #228 bought, on its own rather than
diluted through a whole parse: **1.66x** on an M4 (82.6 us against 136.8 us).
Those numbers are only comparable because `bench/Cargo.lock` is pinned to the
same dependency versions the wheel ships -- the lint job checks it, and the first
version of this crate resolved `data-encoding 2.11.1` against the shipped 2.6.0
and reported 1.86x.

`cargo test --release` in `bench/` counts allocations per mode with a global
allocator, and asserts the claims the modes are documented to make. It is the
reason those claims can be quoted: on `large_message.eml` (785 KiB), a full parse
peaks at **1,060,757 bytes** and metadata mode at **11,929** -- 89x lower, because
it copies no part bodies. Run it with `-- --nocapture` for the whole table.

One result worth knowing before optimising for it: a *lazy* tree holds more than
a decoded one on a small attachment-bearing message (6,696 against 6,323 bytes on
`attachment_message.eml`), because lazy retains the **encoded** bytes and base64
is 4/3 of what it decodes to. The trade only pays once the bodies are large.

### Profiling the shipped codegen

```sh
maturin develop --profile profiling
# or, with no manifest change at all:
CARGO_PROFILE_RELEASE_DEBUG=line-tables-only CARGO_PROFILE_RELEASE_STRIP=none \
  maturin develop --release
```

Same codegen as release, plus line tables, so `samply` or `perf` attributes time
to a line rather than a stripped symbol. Neither changes what `pip install` or
`publish.yml` builds.

A profiling build is a **different binary** from the release one -- line tables
change section sizes and therefore layout, and this crate is measurably sensitive
to layout. Use it to find out *where* the time goes, never how much.

### Is PGO worth it?

Unanswered deliberately, and there is a workflow to answer it:

```sh
gh workflow run pgo-ab.yml -f rounds=5 -f tolerance=5
```

It builds three wheels from one source and one toolchain -- plain, instrumented,
and optimised with the profile that instrumented build produced by running the
whole test suite plus one pass of the benchmark bodies -- then measures plain
against PGO interleaved in a single job, in both orientations so a win announces
itself as loudly as a loss.

**The decision rule matters more than the number.** PGO's mechanism is
rearranging code, and rearranging code is exactly what moves this crate's
benchmarks for no reason at all -- see the layout A/B below, which measured up to
7.5%. So a PGO win counts only if it clears `tolerance`, **exceeds the layout
spread for the same revision**, and reproduces on a run with a different
`Measured on` CPU line. Anything short of that is recorded here as
measured-and-not-adopted, which is a result: it stops the next person re-running
it.

Adopting it would raise a question the workflow does not answer. The PR gate
builds without PGO, so its verdicts would stop describing the artefact users
install; that parity problem belongs in the adoption issue, not the trial.

A trap for local trials on macOS: `.cargo/config.toml` sets `rustflags` for the
apple targets, and a `RUSTFLAGS` environment variable **replaces** it rather than
appending. Exporting `RUSTFLAGS=-Cprofile-generate=...` therefore drops
`-C link-arg=-undefined dynamic_lookup`, and the link fails in a way that looks
nothing like the cause. Add the link args back by hand, or trial it on the linux
runner, where the target has no such entry.

To measure it rather than argue about it, dispatch the layout A/B:

```sh
gh workflow run layout-ab.yml -f salts=4 -f rounds=3
```

Measured on 2026-09-17 across four dispatches: pure placement moves the parse
benchmarks by up to **7.5%** on an Intel Xeon 6973P-C and **7.1%** on an EPYC
9V74, with `parse_qp_message` and `parse_message` sensitive on every CPU tried.
The gate's threshold is 7%, so a verdict at or below that figure on those
benchmarks is not evidence about the code. Trialling
`-C llvm-args=-align-all-nofallthru-blocks=6` as a second group cut
`parse_message`'s spread to 0.8-1.2% and `parse_qp_message`'s to 0.7-1.8% on two
Xeon Platinum parts, at a 2-3% cost to quoted-printable decoding and no
consistent effect on the lazy or metadata paths; it is not adopted.

It builds the same source four times, differing only in a `-C metadata` salt --
the same thing a version bump perturbs (#204) -- and measures all four
interleaved on one runner. The per-benchmark spread across salts is this
revision's layout sensitivity on that CPU, and it is what the gate's 7% threshold
should be read against: a benchmark whose spread is 9% cannot produce a
meaningful 9% verdict. Pass `-f rustflags='-C llvm-args=-align-all-nofallthru-blocks=6'`
to measure an alignment candidate as a second group against it. See
`.github/scripts/layout_spread.py` for the classification rule.

A real regression reproduces on different hardware; a layout-versus-CPU artifact
does not. The gate prints the CPU it measured on for exactly this comparison.

## Linting

The following checks are run in CI. Run them locally before opening a PR:

```bash
cargo fmt --all -- --check       # Rust formatting (blocking in CI)
cargo clippy --all-targets -- -D warnings -W clippy::cast_possible_truncation
mypy --strict fast_mail_parser/  # type-stub checking (blocking in CI)
ruff check --fix .               # Python lint + autofix (blocking in CI; config in ruff.toml)
```

Run **`ruff check --fix .`** before opening a PR — it auto-fixes most findings
(import order, modern typing, etc.); fix any remainder by hand so `ruff check .`
is clean.

In CI:

- **`cargo fmt --check`**, **`mypy --strict`**, and **`ruff check`** are
  **blocking** — keep the source clean under all three.
- **`cargo clippy`** is currently **advisory** (`continue-on-error`) while its
  remaining debt clears; please keep new code clean under it even though it does
  not yet fail the build.

## Continuous integration

The [`Test`](.github/workflows/test.yml) workflow gates every pull request. It
consists of:

- **Lint** — `cargo fmt --check`, `mypy --strict`, and `ruff check` (all
  blocking), plus `cargo clippy` (advisory).
- **cargo audit** — a **blocking** supply-chain audit of the Rust dependency
  stack (PyO3 0.29) against the RustSec advisory database. A new advisory
  against any dependency fails the build.
- **cargo deny** — **blocking**, enforcing `deny.toml`: the licence allowlist,
  advisories, ban rules and source allowlist. Both this and cargo audit set
  `RUSTUP_TOOLCHAIN=stable` to override the toolchain pin, because a
  supply-chain check should see current tooling whatever the library is built
  with.
- **Test matrix** — the real merge gate. Builds and runs the suite on **CPython
  3.11, 3.12, 3.13, and 3.14** via `make install` / `make install-test` /
  `make test`.
- **Benchmark quality gate** — builds **both your revision and its base** in one
  job and compares them, failing if yours is more than
  `BENCH_MAX_REGRESSION_PCT` (7%) slower. Comparing two builds on the same runner
  is what makes a few percent meaningful: within a job the noise is ~0.3%, while
  the same source measured across *different* runners spreads ~26%. An absolute
  ratio against `mail-parser` is still reported and still gated, but only as a
  loose catastrophic-drift net (`BENCH_MIN_SPEEDUP`, 5x) far below the observed
  range — it cannot be tightened without flaking, which is why the base
  comparison exists. Getting faster always passes.

All of these must pass (advisory checks aside) before a PR can merge.

## Pull request conventions

- Keep changes focused; update or add tests for the area you change.
- All commits must be **signed off** under the
  [Developer Certificate of Origin (DCO)](https://developercertificate.org/).
  Add the `Signed-off-by` trailer with:

  ```bash
  git commit -s
  ```

- Make sure `make test` passes and the lint commands above are clean locally
  before pushing.
- If your change alters the public API or observable parsing behavior, update
  the relevant contract / corpus tests and call out the change in your PR
  description.
