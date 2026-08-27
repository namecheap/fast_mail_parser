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
  [`rust-toolchain.toml`](rust-toolchain.toml) to **1.97.1**; if you use
  `rustup`, the correct version is selected automatically in this directory.
  The pin is deliberate: rustc 1.98.0 makes this crate ~26% slower and trips the
  benchmark gate — see the comment in that file before changing it. The MSRV is
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
- It therefore **includes `src/mail_parser.rs` by path**, which works only because
  that module has no PyO3 dependency. If the core ever gains one, this harness
  stops building — which is the right alarm, since the split is what the module
  docs promise.

`fuzz/Cargo.toml` duplicates the `charset` and `mailparse` versions from the root
manifest. They must stay in step, or the harness stops testing what ships.

CI runs a 60-second deterministic pass on every PR, seeded so it stays
reproducible; a crasher is uploaded as an artifact.

A **weekly deep run** (`.github/workflows/deep-fuzz.yml`, Mondays) fuzzes for 30
minutes with no fixed seed and, importantly, **caches the corpus between runs**,
so coverage compounds instead of restarting from the fixtures every week — that
accumulation is most of what makes a scheduled run worth more than the PR pass.

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
workflow with `inject_canary`. It plants an input the target panics on by design
(`CANARY` in `fuzz/fuzz_targets/parse_email.rs`) and the resulting issue says so
— close it afterwards.

A drill **passes green**: with `inject_canary` set, the job succeeds when the
canary crashed and was reported, and fails when it did not, because a canary that
goes undetected means the alarm is broken. A normal run fails on any crash, as
you would expect.

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
