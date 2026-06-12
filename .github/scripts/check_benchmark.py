#!/usr/bin/env python3
"""Performance quality gate for CI.

Reads a pytest-benchmark JSON report and fails the build unless
``fast_mail_parser`` is at least ``BENCH_MIN_SPEEDUP`` times faster than the
pure-Python ``mail-parser`` baseline.

The comparison uses each benchmark's *minimum* time, not the mean: on shared CI
runners the min is the least noise-prone metric (the cleanest observed run), so
the gate is reproducible and does not flake. Both libraries are benchmarked on
the same runner in the same run, so the *ratio* is independent of the runner's
absolute speed (the README reports ~8x). Getting faster always passes; only a
regression below the configured floor fails.

Usage:
    python check_benchmark.py [benchmark.json]

Environment:
    BENCH_MIN_SPEEDUP   Minimum required speedup ratio (default: 4.0).
"""
import json
import os
import sys


def min_for(benchmarks, predicate, label):
    matches = [b for b in benchmarks if predicate(b["name"])]
    if not matches:
        sys.exit(f"::error::benchmark for {label} not found in report")
    if len(matches) > 1:
        names = ", ".join(b["name"] for b in matches)
        sys.exit(f"::error::ambiguous benchmark match for {label}: {names}")
    return matches[0]["stats"]["min"]


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "benchmark.json"
    threshold = float(os.environ.get("BENCH_MIN_SPEEDUP", "4.0"))

    with open(path) as fh:
        benchmarks = json.load(fh)["benchmarks"]

    fast = min_for(
        benchmarks, lambda n: "fast_mail_parser" in n, "fast_mail_parser"
    )
    baseline = min_for(
        benchmarks,
        lambda n: "mail_parser" in n and "fast" not in n,
        "mail-parser (baseline)",
    )

    speedup = baseline / fast
    fast_ms, base_ms = fast * 1e3, baseline * 1e3

    summary = (
        f"### Benchmark quality gate\n\n"
        f"| Library | Min time | Speedup |\n"
        f"|---|---|---|\n"
        f"| fast_mail_parser | {fast_ms:.3f} ms | {speedup:.2f}x |\n"
        f"| mail-parser (baseline) | {base_ms:.3f} ms | 1.00x |\n\n"
        f"Required floor: **{threshold:.2f}x** — "
        f"{'PASS ✅' if speedup >= threshold else 'FAIL ❌'}\n"
    )
    print(summary)
    step_summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if step_summary:
        with open(step_summary, "a") as fh:
            fh.write(summary)

    if speedup < threshold:
        sys.exit(
            f"::error::performance regression: fast_mail_parser is only "
            f"{speedup:.2f}x faster than mail-parser (floor is {threshold:.2f}x)"
        )


if __name__ == "__main__":
    main()
