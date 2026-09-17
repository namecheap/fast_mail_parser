#!/usr/bin/env python3
"""How much of a benchmark's number is code placement rather than code.

`ab_median.py` answers "is B different from A". This answers a question that has
to be settled before that one means anything: **how far does this benchmark move
when nothing changes at all?**

The inputs are several builds of one source that differ only in a layout salt
(`-C metadata=layout<k>`, which feeds the crate hash the way a version bump does,
#204). Their instruction streams are the same work; only addresses differ. So the
spread across salts is not noise to be averaged away -- it is the width of the
band any single A/B verdict on this revision is drawn from, on this CPU.

Read it against the gate's 7%: if a benchmark's spread is 9%, a 9% verdict on it
says nothing, and the gate is measuring the linker.

With two groups (`plain` and an `aligned` build tried against it) a third table
gives `cost(b)` -- what the alignment flags did to the median -- so the decision
is "does it shrink the spread, and what does it cost" rather than a guess.

Exit code is 0 for any successful measurement: this is a dispatch-only
instrument, and the decision is a human's. Same policy as `ab_median.py`.
"""

import argparse
import os
import statistics
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from ab_median import is_control, read_machine, read_mins  # noqa: E402

# Below this, a salt sweep cannot separate placement from the residual it sits
# in: #205 measured the version-bump effect at 3.2-4.4% on the gate's own CPU.
# Reporting 2% as "sensitive" would be pretending to a resolution this has not
# got.
MIN_SENSITIVE_PCT = 3.0

# Matches the gate (`test.yml` passes `--informational test__threaded___`), so
# the two instruments classify the same benchmarks the same way.
INFORMATIONAL_PREFIX = "test__threaded___"

# Below this, a percentage is arithmetic on noise. `attachment_reread` and
# `headers_repeat_read` run in tens of nanoseconds -- they exist to show that a
# cached read costs nothing, which is the point of them -- and at that scale the
# timer's own granularity is a large fraction of the measurement. The first run
# of this script reported them at 14% and 15% "spread" and named them
# layout-sensitive, which was quantisation, not placement. A build decision was
# about to be read off that table, so they are now excluded from the
# classification and labelled instead of silently dropped.
MIN_SENSITIVE_SECONDS = 1e-6


def side_of(path):
    """`plain-2-3.json` -> `2`. The salt index, not the round."""
    stem = os.path.basename(path).rsplit(".", 1)[0]
    parts = stem.rsplit("-", 2)
    if len(parts) != 3:
        sys.exit(f"::error::{path}: expected <group>-<salt>-<round>.json")
    return parts[1]


def medians_per_side(paths):
    """{benchmark: {salt: median over rounds of that run's minimum}}."""
    by_side = {}
    for path in paths:
        for name, value in read_mins(path).items():
            by_side.setdefault(name, {}).setdefault(side_of(path), []).append(value)
    return {
        name: {salt: statistics.median(values) for salt, values in sides.items()}
        for name, sides in by_side.items()
    }


def spread_pct(per_salt):
    """Widest disagreement between salts, as a percentage of the fastest."""
    lo, hi = min(per_salt.values()), max(per_salt.values())
    if lo <= 0:
        return 0.0
    return (hi / lo - 1) * 100


def emit(lines):
    text = "\n".join(lines)
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as fh:
            fh.write(text + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--group",
        nargs="+",
        action="append",
        required=True,
        metavar=("LABEL", "REPORT"),
        help="a group label followed by its <group>-<salt>-<round>.json reports",
    )
    args = parser.parse_args()

    groups = {}
    for group in args.group:
        if len(group) < 2:
            sys.exit("::error::--group needs a label and at least one report")
        label, paths = group[0], group[1:]
        groups[label] = medians_per_side(paths)

    first = next(iter(groups.values()))
    if not any(is_control(name) for name in first):
        # Without a control there is no floor, and every spread below is
        # uninterpretable -- it could all be the runner.
        sys.exit(
            "::error::no control benchmark in the reports; the spread has no floor "
            "to be read against. Do not filter out test__mail_parser___, "
            "test__mailparser_lib___ or test__stdlib_email___."
        )

    machine = read_machine(args.group[0][1])
    lines = ["## Layout spread", "", f"Measured on `{machine}`.", ""]
    lines += [
        "Each side is the same source built with a different `-C metadata` salt, "
        "so the sides do the same work and differ only in where the code landed. "
        "`spread` is the widest disagreement between salts.",
        "",
    ]

    floors = {}
    for label, benches in groups.items():
        salts = sorted({salt for per_salt in benches.values() for salt in per_salt})
        floors[label] = max(
            (spread_pct(per_salt) for name, per_salt in benches.items() if is_control(name)),
            default=0.0,
        )
        lines += [
            f"### `{label}`",
            "",
            "| Benchmark | " + " | ".join(f"salt {s}" for s in salts) + " | spread | |",
            "|---" * (len(salts) + 3) + "|",
        ]
        for name in sorted(benches):
            per_salt = benches[name]
            cells = " | ".join(
                f"{per_salt[s] * 1e3:.3f}" if s in per_salt else "--" for s in salts
            )
            spread = spread_pct(per_salt)
            if is_control(name):
                tag = "control"
            elif name.startswith(INFORMATIONAL_PREFIX):
                tag = "informational"
            elif max(per_salt.values()) < MIN_SENSITIVE_SECONDS:
                tag = "too fast to classify"
            elif spread > floors[label] and spread > MIN_SENSITIVE_PCT:
                tag = "**layout-sensitive**"
            else:
                tag = ""
            lines.append(f"| `{name}` | {cells} | {spread:+.1f}% | {tag} |")
        lines += [
            "",
            f"Control floor: **{floors[label]:.1f}%** -- the controls are pure Python "
            "and cannot be affected by where the Rust landed, so this is what the "
            "runner itself contributes.",
            "",
        ]
        sensitive = [
            name
            for name, per_salt in benches.items()
            if not is_control(name)
            and not name.startswith(INFORMATIONAL_PREFIX)
            and max(per_salt.values()) >= MIN_SENSITIVE_SECONDS
            and spread_pct(per_salt) > floors[label]
            and spread_pct(per_salt) > MIN_SENSITIVE_PCT
        ]
        if sensitive:
            worst = max(spread_pct(benches[name]) for name in sensitive)
            lines += [
                f"**{len(sensitive)} benchmark(s) are layout-sensitive**, worst "
                f"**{worst:.1f}%**. A single A/B verdict on those, at or below that "
                "figure, is not evidence about the code.",
                "",
            ]
        else:
            lines += [
                "No benchmark's spread clears both the control floor and "
                f"{MIN_SENSITIVE_PCT:.0f}%. On this CPU, placement is not what the "
                "gate's verdicts are made of.",
                "",
            ]

    if len(groups) == 2:
        (base_label, base), (cand_label, cand) = groups.items()
        lines += [
            f"### `{cand_label}` against `{base_label}`",
            "",
            "| Benchmark | " + f"{base_label} spread | {cand_label} spread | cost |",
            "|---|---|---|---|",
        ]
        for name in sorted(set(base) & set(cand)):
            base_median = statistics.median(base[name].values())
            cand_median = statistics.median(cand[name].values())
            cost = (cand_median / base_median - 1) * 100 if base_median else 0.0
            lines.append(
                f"| `{name}` | {spread_pct(base[name]):+.1f}% | "
                f"{spread_pct(cand[name]):+.1f}% | {cost:+.1f}% |"
            )
        lines += [
            "",
            "`cost` is what the extra flags did to the median. The trade is worth "
            "making only if the spread shrinks by more than the cost.",
            "",
        ]

    emit(lines)


if __name__ == "__main__":
    main()
