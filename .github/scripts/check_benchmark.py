#!/usr/bin/env python3
"""Performance quality gate for CI.

Reads pytest-benchmark JSON reports and fails the build on a performance
regression. Two checks, in order of authority:

1. **Regression against the comparison base** (primary). When a base report is
   supplied, ``fast_mail_parser`` must not be more than
   ``BENCH_MAX_REGRESSION_PCT`` slower than the same benchmark run against the
   base revision's build, measured **in the same job on the same runner**.

2. **Absolute sanity floor** (secondary). ``fast_mail_parser`` must still be at
   least ``BENCH_MIN_SPEEDUP`` times faster than the pure-Python
   ``mail-parser``. This is a catastrophic-drift net, not the real gate -- see
   below for why it cannot be tight.

Why the base comparison is the primary check
--------------------------------------------
The absolute ratio was the gate, and it flaked. Measured on this project:

- Within a single job, four consecutive runs of the same binary spread **0.3%**
  (1.895-1.902 ms).
- Between jobs, the same source spread **26%** (1.885-2.378 ms), while the
  pure-Python baseline barely moved (14.2-14.5 ms).

So the noise is between runner VMs, not in the measurement, and the two
implementations do not scale together -- the Rust extension is far more
sensitive to the runner's CPU than the interpreted baseline. An absolute floor
therefore sits inside the between-runner noise band: tight enough to catch a
real regression means flaking on honest changes, and loose enough not to flake
means missing the ~26% class of regression that motivated this (see issue #120).

Comparing two builds in the same job cancels that, because both measurements see
the same CPU in the same thermal state. At 0.3% noise a few-percent threshold is
meaningful.

The comparison uses each benchmark's *minimum* time, not the mean: the min is
the cleanest observed run and the least noise-prone metric.

Usage:
    python check_benchmark.py HEAD.json [BASE.json]

Environment:
    BENCH_MAX_REGRESSION_PCT  Max tolerated slowdown vs base, percent (default: 7).
    BENCH_MIN_SPEEDUP         Absolute sanity floor vs mail-parser (default: 5.0).
"""
import json
import os
import sys


def _min_for(benchmarks, predicate, label, path):
    matches = [b for b in benchmarks if predicate(b["name"])]
    if not matches:
        sys.exit(f"::error::benchmark for {label} not found in {path}")
    if len(matches) > 1:
        names = ", ".join(b["name"] for b in matches)
        sys.exit(f"::error::ambiguous benchmark match for {label} in {path}: {names}")
    return matches[0]["stats"]["min"]


def read_report(path):
    """Return (fast_seconds, baseline_seconds) from a pytest-benchmark report."""
    with open(path) as fh:
        benchmarks = json.load(fh)["benchmarks"]
    fast = _min_for(
        benchmarks, lambda n: "fast_mail_parser" in n, "fast_mail_parser", path
    )
    baseline = _min_for(
        benchmarks,
        lambda n: "mail_parser" in n and "fast" not in n,
        "mail-parser (baseline)",
        path,
    )
    return fast, baseline


def emit(summary):
    print(summary)
    step_summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if step_summary:
        with open(step_summary, "a") as fh:
            fh.write(summary)


def main():
    if len(sys.argv) < 2:
        sys.exit("::error::usage: check_benchmark.py HEAD.json [BASE.json]")

    head_path = sys.argv[1]
    base_path = sys.argv[2] if len(sys.argv) > 2 else None

    max_regression = float(os.environ.get("BENCH_MAX_REGRESSION_PCT", "7"))
    floor = float(os.environ.get("BENCH_MIN_SPEEDUP", "5.0"))

    head_fast, head_base = read_report(head_path)
    head_ratio = head_base / head_fast

    lines = [
        "### Benchmark quality gate\n",
        "| Revision | fast_mail_parser | mail-parser | Ratio |",
        "|---|---|---|---|",
        f"| this revision | {head_fast * 1e3:.3f} ms | {head_base * 1e3:.3f} ms "
        f"| {head_ratio:.2f}x |",
    ]

    failures = []

    if base_path:
        base_fast, base_baseline = read_report(base_path)
        lines.append(
            f"| comparison base | {base_fast * 1e3:.3f} ms "
            f"| {base_baseline * 1e3:.3f} ms | {base_baseline / base_fast:.2f}x |"
        )
        # Positive = this revision is slower than the base.
        delta_pct = (head_fast / base_fast - 1.0) * 100
        verdict = "PASS ✅" if delta_pct <= max_regression else "FAIL ❌"
        lines.append(
            f"\n**Change vs base: {delta_pct:+.1f}%** "
            f"(tolerated: +{max_regression:.0f}%) — {verdict}\n"
        )
        lines.append(
            "_Both revisions are built and measured in this job on this runner, "
            "so between-runner variance (~26% historically) does not enter the "
            "comparison; within-job noise is ~0.3%._\n"
        )
        if delta_pct > max_regression:
            failures.append(
                f"performance regression: this revision is {delta_pct:+.1f}% "
                f"slower than the base (tolerated +{max_regression:.0f}%)"
            )
    else:
        lines.append(
            "\n_No base report supplied — regression check skipped, absolute "
            "floor only._\n"
        )

    floor_verdict = "PASS ✅" if head_ratio >= floor else "FAIL ❌"
    lines.append(
        f"Absolute sanity floor: **{floor:.2f}x** vs mail-parser — {floor_verdict}\n"
    )
    if head_ratio < floor:
        failures.append(
            f"absolute floor breached: only {head_ratio:.2f}x faster than "
            f"mail-parser (floor is {floor:.2f}x)"
        )

    emit("\n".join(lines) + "\n")

    for failure in failures:
        print(f"::error::{failure}", file=sys.stderr)
    if failures:
        sys.exit(1)


if __name__ == "__main__":
    main()
