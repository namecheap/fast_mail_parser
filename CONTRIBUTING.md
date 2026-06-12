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

- **Rust** — the toolchain is pinned in [`rust-toolchain`](rust-toolchain) to
  **1.83**; if you use `rustup`, the correct version is selected automatically
  in this directory. Some CI checks (`cargo audit`) run on **stable** rather
  than the pinned version.
- **Python** — **3.9–3.13** (`requires-python = ">= 3.9"` in
  [`pyproject.toml`](pyproject.toml)). The CI test matrix covers 3.9, 3.10,
  3.11, 3.12, and 3.13.
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
  contract**: the exported names (`parse_email`, `ParseError`, `PyMail`,
  `PyAttachment`), the attribute set and types of `PyMail` / `PyAttachment`, the
  input types `parse_email` accepts (`str` and `bytes`), and the errors it
  raises. A failure here means a consumer-visible change.
- [`tests/test_rfc_corpus.py`](tests/test_rfc_corpus.py) — characterization
  tests over an **RFC-feature `.eml` corpus** in `tests/data/rfc/`, locking the
  parser's actual output per email/MIME RFC feature (multipart, base64,
  quoted-printable, RFC 2047/2231/6532, folded headers, etc.).
- [`tests/test_contents.py`](tests/test_contents.py),
  [`tests/test_attachments.py`](tests/test_attachments.py),
  [`tests/test_headers.py`](tests/test_headers.py),
  [`tests/test_empty_fields.py`](tests/test_empty_fields.py) — focused tests on
  body text, attachments, headers, and empty-field handling.

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

The benchmark compares `fast_mail_parser` against the pure-Python `mail-parser`
baseline. Build with `--release` first, then:

```bash
maturin develop --release
make benchmark
```

## Linting

The following checks are run in CI. Run them locally before opening a PR:

```bash
cargo fmt --all -- --check       # Rust formatting (enforced / blocking in CI)
cargo clippy --all-targets -- -D warnings -W clippy::cast_possible_truncation
mypy fast_mail_parser/           # type-stub checking (advisory in CI)
ruff check .                     # Python linting (advisory)
```

In CI:

- **`cargo fmt --check`** is **blocking** — the source is rustfmt-clean and must
  stay that way.
- **`cargo clippy`** and **`mypy`** are currently **advisory**
  (`continue-on-error`) while their remaining debt clears. Please keep new code
  clean under both even though they do not yet fail the build.
- `ruff` is advisory; keep Python code clean under it.

## Continuous integration

The [`Test`](.github/workflows/test.yml) workflow gates every pull request. It
consists of:

- **Lint** — `cargo fmt --check` (blocking), plus `cargo clippy` and `mypy`
  (advisory).
- **cargo audit** — a **blocking** supply-chain audit of the Rust dependency
  stack (PyO3 0.29) against the RustSec advisory database. A new advisory
  against any dependency fails the build.
- **Test matrix** — the real merge gate. Builds and runs the suite on **CPython
  3.9, 3.10, 3.11, 3.12, and 3.13** via `make install` / `make install-test` /
  `make test`.
- **Benchmark quality gate** — runs the benchmark once and enforces that
  `fast_mail_parser` stays at least **8x** faster than the pure-Python
  `mail-parser` baseline (`BENCH_MIN_SPEEDUP`). The ratio is measured on the same
  runner in the same run, so it is hardware-independent; getting faster always
  passes, only a regression below the floor fails.

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
