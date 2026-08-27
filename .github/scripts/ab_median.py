#!/usr/bin/env python3
"""Compare two builds measured in interleaved rounds.

``check_benchmark.py`` compares one measurement per side, which is right for the
PR gate: there, both sides are built and measured once in the same job, and
within-job noise is ~0.3%.

That does not hold when the two sides need *separate build-and-measure cycles*.
Measured on this project, two such cycles read ~16% apart on a no-op change --
the build cycle itself moves the number, presumably via cache and thermal state.
A one-shot A/B therefore cannot resolve a threshold anywhere near 5%: it will
report a confident number that means nothing.

The fix is to stop pairing a build with its measurement. Build both wheels
first, then alternate measurement rounds between the two installed builds
(A, B, A, B, ...) and take each side's **median across rounds**. Interleaving
cancels monotonic drift, and the median discards a single disturbed round.

The pure-Python benchmarks are the noise floor. They cannot be affected by how
the Rust extension was built, so whatever delta they show is measurement error,
and a treatment delta is only believable when it clears that floor. This is the
control that made the toolchain measurement trustworthy (#120): the effect was
+30% while the control moved +2.3%.

Usage:
    ab_median.py --a-label abi3 --b-label versioned \
        --a a1.json a2.json a3.json --b b1.json b2.json b3.json

Environment:
    AB_THRESHOLD_PCT  Delta above which the verdict is "significant" (default: 5).
"""
import argparse
import json
import os
import statistics
import sys

# Benchmarks that cannot be affected by how the extension was built. Their
# spread is the measurement noise floor for the run.
CONTROL_PREFIXES = (
    "test__mail_parser___",
    "test__mailparser_lib___",
    "test__stdlib_email___",
)


def read_machine(path):
    """The hardware a report was produced on.

    Reported because the same two binaries can measure identically on one runner
    and 96% apart on another: code layout interacts with the microarchitecture,
    and GitHub's fleet is not homogeneous (#204). Without the CPU printed, two
    runs of the same commit cannot be compared at all.
    """
    with open(path) as fh:
        info = json.load(fh).get("machine_info", {})
    cpu = info.get("cpu", {})
    brand = cpu.get("brand_raw") or info.get("processor") or "unknown CPU"
    count = cpu.get("count")
    return f"{brand}" + (f", {count} vCPU" if count else "")


def read_mins(path):
    """Return {benchmark name: minimum seconds} from a pytest-benchmark report."""
    with open(path) as fh:
        report = json.load(fh)
    mins = {}
    for bench in report["benchmarks"]:
        name = bench["name"]
        if name in mins:
            sys.exit(f"::error::benchmark {name!r} appears more than once in {path}")
        mins[name] = bench["stats"]["min"]
    if not mins:
        sys.exit(f"::error::no benchmarks in {path}")
    return mins


def is_control(name):
    return name.startswith(CONTROL_PREFIXES)


def classify(name, informational):
    """One of "control", "informational", or "treatment"."""
    if is_control(name):
        return "control"
    if informational and name.startswith(informational):
        return "informational"
    return "treatment"


def collect(paths):
    """Return ({name: [per-round seconds]}, round_count)."""
    rounds = [read_mins(path) for path in paths]
    names = set(rounds[0])
    for extra in rounds[1:]:
        names &= set(extra)
    dropped = sorted((set().union(*(set(r) for r in rounds))) - names)
    if dropped:
        print(f"::warning::benchmarks missing from some rounds, ignored: {dropped}")
    return {name: [r[name] for r in rounds] for name in sorted(names)}, len(rounds)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--a-label", required=True)
    parser.add_argument("--b-label", required=True)
    parser.add_argument("--a", nargs="+", required=True, metavar="JSON")
    parser.add_argument("--b", nargs="+", required=True, metavar="JSON")
    # Some benchmarks are worth running on every PR and worth nobody's build
    # failing over. A benchmark that spawns threads is the case in hand: thread
    # scheduling moves it far more than a code change would, so gating on it buys
    # flakes rather than protection. Reported, not enforced -- and running, which
    # is what keeps it from rotting.
    parser.add_argument(
        "--informational",
        nargs="+",
        default=(),
        metavar="PREFIX",
        help="benchmark name prefixes to report but exclude from the verdict",
    )
    args = parser.parse_args()
    informational = tuple(args.informational)

    threshold = float(os.environ.get("AB_THRESHOLD_PCT", "5"))

    a_rounds, a_count = collect(args.a)
    b_rounds, b_count = collect(args.b)
    shared = sorted(set(a_rounds) & set(b_rounds))
    if not shared:
        sys.exit("::error::the two sides share no benchmarks")

    lines = [
        f"### A/B: {args.a_label} vs {args.b_label}\n",
        f"Measured on `{read_machine(args.a[0])}`.\n",
        f"Median of {a_count} interleaved rounds per side; each value is a "
        "benchmark's minimum. Positive delta = "
        f"`{args.a_label}` is slower.\n",
        f"| Benchmark | {args.b_label} | {args.a_label} | Delta | |",
        "|---|---|---|---|---|",
    ]

    deltas = {}
    kinds = {}
    for name in shared:
        a_median = statistics.median(a_rounds[name])
        b_median = statistics.median(b_rounds[name])
        delta = (a_median / b_median - 1.0) * 100
        deltas[name] = delta
        kinds[name] = classify(name, informational)
        tag = "" if kinds[name] == "treatment" else kinds[name]
        lines.append(
            f"| `{name}` | {b_median * 1e3:.3f} ms | {a_median * 1e3:.3f} ms "
            f"| {delta:+.1f}% | {tag} |"
        )

    controls = {n: d for n, d in deltas.items() if kinds[n] == "control"}
    treatments = {n: d for n, d in deltas.items() if kinds[n] == "treatment"}
    if not treatments:
        sys.exit(
            "::error::no benchmark is left to judge -- every one matched a "
            "control or an informational prefix"
        )

    noise_floor = max((abs(d) for d in controls.values()), default=None)
    worst_name, worst = max(treatments.items(), key=lambda kv: kv[1])

    lines.append("")
    if noise_floor is None:
        lines.append(
            "_No control benchmark present, so there is no in-run noise "
            "estimate: treat the deltas as unbounded._\n"
        )
    else:
        lines.append(
            f"Noise floor from the pure-Python controls: **{noise_floor:.1f}%** "
            "(they cannot be affected by the build, so this is measurement "
            "error).\n"
        )

    lines.append(f"Worst treatment delta: **{worst:+.1f}%** (`{worst_name}`).\n")

    significant = worst > threshold and (noise_floor is None or worst > noise_floor)
    if significant:
        lines.append(
            f"**Verdict: significant.** `{args.a_label}` is {worst:+.1f}% slower "
            f"than `{args.b_label}`, past the {threshold:.0f}% threshold and "
            "clear of the noise floor.\n"
        )
        # Not hedging a real result -- naming a failure mode this gate cannot
        # otherwise detect. The controls are pure Python and do not care how the
        # extension was laid out, so they stay flat while a layout-versus-CPU
        # difference moves every treatment benchmark tightly and repeatably.
        lines.append(
            "**Re-run before acting on this.** Two builds differing only in a "
            "version string measured within 0.4% of each other on one runner and "
            "**96% apart** on another, with per-round spread under 0.3% both "
            "times and identical binaries (#204). A real regression reproduces on "
            "different hardware; a code-layout artifact does not. Compare the CPU "
            "line above between the two runs.\n"
        )
    elif worst > threshold:
        lines.append(
            f"**Verdict: inconclusive.** The {worst:+.1f}% delta exceeds the "
            f"{threshold:.0f}% threshold but does not clear the "
            f"{noise_floor:.1f}% noise floor, so this run cannot tell the "
            "effect from measurement error. Re-run with more rounds.\n"
        )
    else:
        lines.append(
            f"**Verdict: no significant difference.** Worst delta {worst:+.1f}% "
            f"is within the {threshold:.0f}% threshold.\n"
        )

    lines.append("<details><summary>Per-round values (ms)</summary>\n")
    for label, rounds in ((args.a_label, a_rounds), (args.b_label, b_rounds)):
        lines.append(f"\n`{label}`:\n")
        for name in shared:
            series = " ".join(f"{v * 1e3:.3f}" for v in rounds[name])
            lines.append(f"- `{name}`: {series}")
    lines.append("\n</details>\n")

    summary = "\n".join(lines) + "\n"
    print(summary)
    step_summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if step_summary:
        with open(step_summary, "a") as fh:
            fh.write(summary)

    # Exit nonzero only when the difference is real: a dispatch-only measurement
    # workflow going red is the signal that a decision is needed, not a failure.
    if significant:
        print(
            f"::error::{args.a_label} is {worst:+.1f}% slower than "
            f"{args.b_label} — a decision is required",
            file=sys.stderr,
        )
        sys.exit(1)


if __name__ == "__main__":
    main()
