"""Tests for the interleaved A/B comparator used to make build decisions.

``.github/scripts/ab_median.py`` decides whether a measured difference between
two builds is real. Its noise-floor rule is the subtle part, and a comparator
that silently mis-decides is worse than none: it produces a confident number
that a decision then rests on. So the rule is pinned here.
"""
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / ".github" / "scripts" / "ab_median.py"

pytestmark = pytest.mark.skipif(
    not SCRIPT.exists(), reason="CI scripts are not part of the installed package"
)


def _report(path: Path, treatment: float, control: float) -> str:
    """Write a minimal pytest-benchmark report. Times are in seconds."""
    path.write_text(
        json.dumps(
            {
                "benchmarks": [
                    {
                        "name": "test__fast_mail_parser___parse_message",
                        "stats": {"min": treatment},
                    },
                    {
                        "name": "test__mail_parser___parse_message",
                        "stats": {"min": control},
                    },
                ]
            }
        )
    )
    return str(path)


def _report_named(path: Path, values: dict) -> str:
    """Write a report with arbitrary benchmark names. Times are in seconds."""
    path.write_text(
        json.dumps(
            {
                "benchmarks": [
                    {"name": name, "stats": {"min": seconds}}
                    for name, seconds in values.items()
                ]
            }
        )
    )
    return str(path)


def _run(tmp_path: Path, a_rounds, b_rounds, threshold="5"):
    a = [
        _report(tmp_path / f"a{i}.json", t, c) for i, (t, c) in enumerate(a_rounds)
    ]
    b = [
        _report(tmp_path / f"b{i}.json", t, c) for i, (t, c) in enumerate(b_rounds)
    ]
    env = os.environ.copy()
    env["AB_THRESHOLD_PCT"] = threshold
    # Otherwise a test run inside CI would append its synthetic verdicts to the
    # job summary.
    env.pop("GITHUB_STEP_SUMMARY", None)
    return subprocess.run(
        [sys.executable, str(SCRIPT), "--a-label", "abi3", "--b-label", "versioned",
         "--a", *a, "--b", *b],
        capture_output=True,
        text=True,
        env=env,
    )


# Steady control, sides equal.
EQUAL = [(1.000, 10.0), (1.010, 10.1), (0.995, 9.90)]
BASE = [(1.005, 10.0), (0.998, 10.0), (1.002, 10.05)]


def test__no_difference_passes(tmp_path: Path):
    result = _run(tmp_path, EQUAL, BASE)

    assert result.returncode == 0, result.stdout
    assert "no significant difference" in result.stdout


def test__real_effect_over_a_steady_control_is_significant(tmp_path: Path):
    slower = [(1.120, 10.0), (1.125, 10.1), (1.118, 9.95)]

    result = _run(tmp_path, slower, BASE)

    # Nonzero exit is how a dispatch-only measurement says "decide something".
    assert result.returncode == 1
    assert "Verdict: significant" in result.stdout
    # median(1.120, 1.125, 1.118) / median(1.005, 0.998, 1.002) - 1
    assert "+11.8%" in result.stdout


def test__effect_inside_the_noise_floor_is_inconclusive_not_significant(
    tmp_path: Path,
):
    # 8% apparent cost, but the pure-Python control moved 20% -- which it cannot
    # do for any reason connected to the build. The run cannot tell the effect
    # from measurement error, and must not claim it can.
    noisy = [(1.080, 12.0), (1.085, 12.1), (1.078, 11.9)]

    result = _run(tmp_path, noisy, BASE)

    assert result.returncode == 0
    assert "Verdict: inconclusive" in result.stdout
    assert "Verdict: significant" not in result.stdout


def test__median_discards_one_disturbed_round(tmp_path: Path):
    # One round arrives 3x slow (a noisy neighbour on the runner). The median
    # ignores it; a mean would report a ~65% regression that is not there.
    disturbed = [(1.000, 10.0), (3.000, 30.0), (1.005, 10.0)]

    result = _run(tmp_path, disturbed, BASE)

    assert result.returncode == 0, result.stdout
    assert "no significant difference" in result.stdout


def test__a_faster_side_is_not_reported_as_a_regression(tmp_path: Path):
    faster = [(0.800, 10.0), (0.805, 10.1), (0.798, 9.95)]

    result = _run(tmp_path, faster, BASE)

    assert result.returncode == 0
    assert "no significant difference" in result.stdout


# --- informational benchmarks -------------------------------------------------


def _run_named(tmp_path: Path, a_values, b_values, extra_args=()):
    a = _report_named(tmp_path / "an.json", a_values)
    b = _report_named(tmp_path / "bn.json", b_values)
    env = os.environ.copy()
    env["AB_THRESHOLD_PCT"] = "5"
    env.pop("GITHUB_STEP_SUMMARY", None)
    return subprocess.run(
        [sys.executable, str(SCRIPT), "--a-label", "a", "--b-label", "b",
         *extra_args, "--a", a, "--b", b],
        capture_output=True,
        text=True,
        env=env,
    )


def test__an_informational_benchmark_does_not_decide_the_verdict(tmp_path: Path):
    # The threaded benchmark swings 40%; the gated one does not move. Without the
    # exclusion this fails the build on scheduling noise.
    a = {
        "test__fast_mail_parser___parse_message": 1.000,
        "test__threaded___parse_many": 1.400,
        "test__mail_parser___parse_message": 10.0,
    }
    b = {
        "test__fast_mail_parser___parse_message": 1.000,
        "test__threaded___parse_many": 1.000,
        "test__mail_parser___parse_message": 10.0,
    }

    result = _run_named(tmp_path, a, b, ("--informational", "test__threaded___"))

    assert result.returncode == 0, result.stdout
    assert "no significant difference" in result.stdout
    # Reported, not hidden: it is still in the table, labelled.
    assert "test__threaded___parse_many" in result.stdout
    assert "informational" in result.stdout


def test__without_the_exclusion_the_same_data_fails(tmp_path: Path):
    # Guards the exclusion against being a no-op.
    a = {
        "test__fast_mail_parser___parse_message": 1.000,
        "test__threaded___parse_many": 1.400,
        "test__mail_parser___parse_message": 10.0,
    }
    b = {
        "test__fast_mail_parser___parse_message": 1.000,
        "test__threaded___parse_many": 1.000,
        "test__mail_parser___parse_message": 10.0,
    }

    result = _run_named(tmp_path, a, b)

    assert result.returncode == 1
    assert "Verdict: significant" in result.stdout


def test__a_significant_verdict_tells_the_reader_to_re_run(tmp_path: Path):
    # The gate has a false-positive mode its own noise floor cannot see: a code
    # layout difference costs nothing on one CPU and ~2x on another, tightly and
    # repeatably, while the pure-Python controls stay flat. The only cheap defence
    # is re-running, so a failure has to say so.
    slower = [(1.500, 10.0), (1.505, 10.1), (1.498, 9.95)]

    result = _run(tmp_path, slower, BASE)

    assert result.returncode == 1
    assert "Re-run before acting on this" in result.stdout
    assert "96% apart" in result.stdout


def test__the_machine_is_reported(tmp_path: Path):
    # Two runs of the same commit cannot be compared without knowing whether they
    # ran on the same hardware.
    result = _run(tmp_path, EQUAL, BASE)

    assert "Measured on" in result.stdout
