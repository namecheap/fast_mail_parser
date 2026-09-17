"""Tests for the layout-spread instrument.

``.github/scripts/layout_spread.py`` exists to say how much of a benchmark's
number is where the code landed rather than what the code does. Its whole value
is the classification rule -- a spread has to clear both the control floor and an
absolute floor before it is called layout-sensitive -- and an instrument that
mis-classifies is worse than none, because a build decision then rests on it. So
the rule is pinned here, case by case.
"""
import json
import subprocess
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / ".github" / "scripts" / "layout_spread.py"

pytestmark = pytest.mark.skipif(
    not SCRIPT.exists(), reason="CI scripts are not part of the installed package"
)

TREATMENT = "test__fast_mail_parser___parse_message"
CONTROL = "test__mail_parser___parse_message"
INFORMATIONAL = "test__threaded___parse_many"


def _report(path: Path, benchmarks: dict, cpu: str = "AMD EPYC 7763") -> None:
    """A minimal pytest-benchmark report. Times are seconds."""
    path.write_text(
        json.dumps(
            {
                "machine_info": {"cpu": {"brand_raw": cpu, "count": 4}},
                "benchmarks": [
                    {"name": name, "stats": {"min": value}}
                    for name, value in benchmarks.items()
                ],
            }
        )
    )


def _write_group(tmp_path: Path, label: str, per_salt: list[dict], rounds: int = 2) -> list[str]:
    paths = []
    for salt, benchmarks in enumerate(per_salt):
        for rnd in range(1, rounds + 1):
            path = tmp_path / f"{label}-{salt}-{rnd}.json"
            _report(path, benchmarks)
            paths.append(str(path))
    return paths


def _run(*args: str, expect_ok: bool = True) -> str:
    result = subprocess.run(
        [sys.executable, str(SCRIPT), *args], capture_output=True, text=True
    )
    if expect_ok:
        assert result.returncode == 0, result.stderr
    else:
        assert result.returncode != 0, result.stdout
    return result.stdout + result.stderr


def _row(output: str, name: str) -> str:
    for line in output.splitlines():
        if line.startswith(f"| `{name}`"):
            return line
    raise AssertionError(f"no row for {name} in:\n{output}")


# --- (a) nothing differs ------------------------------------------------------


def test__identical_sides_report_no_spread_and_nothing_sensitive(tmp_path: Path):
    sides = [{TREATMENT: 0.001, CONTROL: 0.01} for _ in range(4)]
    output = _run("--group", "plain", *_write_group(tmp_path, "plain", sides))

    assert "+0.0%" in _row(output, TREATMENT)
    assert "layout-sensitive" not in _row(output, TREATMENT)
    assert "placement is not what the gate's verdicts are made of" in output


# --- (b) a real spread --------------------------------------------------------


def test__a_treatment_spread_above_both_floors_is_classified_sensitive(tmp_path: Path):
    sides = [
        {TREATMENT: 0.001, CONTROL: 0.01},
        {TREATMENT: 0.001, CONTROL: 0.01},
        {TREATMENT: 0.0011, CONTROL: 0.01},  # +10%
        {TREATMENT: 0.001, CONTROL: 0.01},
    ]
    output = _run("--group", "plain", *_write_group(tmp_path, "plain", sides))

    row = _row(output, TREATMENT)
    assert "+10.0%" in row
    assert "layout-sensitive" in row
    assert "worst **10.0%**" in output


# --- (c) the control floor rule ----------------------------------------------


def test__a_spread_under_the_control_floor_is_not_sensitive(tmp_path: Path):
    # Same +10% treatment spread as (b), but the runner itself moved 12% -- so
    # the treatment figure says nothing the controls do not already say.
    sides = [
        {TREATMENT: 0.001, CONTROL: 0.01},
        {TREATMENT: 0.001, CONTROL: 0.01},
        {TREATMENT: 0.0011, CONTROL: 0.0112},
        {TREATMENT: 0.001, CONTROL: 0.01},
    ]
    output = _run("--group", "plain", *_write_group(tmp_path, "plain", sides))

    assert "layout-sensitive" not in _row(output, TREATMENT)
    assert "Control floor: **12.0%**" in output


# --- (d) the absolute floor rule ---------------------------------------------


def test__a_small_spread_over_a_tiny_floor_is_not_sensitive(tmp_path: Path):
    # 2.5% clears a 0.1% control floor but not the 3% absolute floor: a four-salt
    # sweep cannot separate that from the residual it sits in.
    sides = [
        {TREATMENT: 0.001, CONTROL: 0.01},
        {TREATMENT: 0.001025, CONTROL: 0.01001},
        {TREATMENT: 0.001, CONTROL: 0.01},
        {TREATMENT: 0.001, CONTROL: 0.01},
    ]
    output = _run("--group", "plain", *_write_group(tmp_path, "plain", sides))

    row = _row(output, TREATMENT)
    assert "+2.5%" in row
    assert "layout-sensitive" not in row


# --- (e) two groups -----------------------------------------------------------


def test__two_groups_report_the_cost_of_the_extra_flags(tmp_path: Path):
    plain = [{TREATMENT: 0.001, CONTROL: 0.01} for _ in range(2)]
    aligned = [{TREATMENT: 0.0012, CONTROL: 0.01} for _ in range(2)]
    output = _run(
        "--group", "plain", *_write_group(tmp_path, "plain", plain),
        "--group", "aligned", *_write_group(tmp_path, "aligned", aligned),
    )

    assert "### `aligned` against `plain`" in output
    cost_row = [
        line for line in output.splitlines()
        if line.startswith(f"| `{TREATMENT}`") and line.count("%") == 3
    ]
    assert cost_row, output
    assert "+20.0%" in cost_row[0]


# --- (f) the CPU is reported --------------------------------------------------


def test__the_cpu_is_reported(tmp_path: Path):
    sides = [{TREATMENT: 0.001, CONTROL: 0.01} for _ in range(2)]
    output = _run("--group", "plain", *_write_group(tmp_path, "plain", sides))

    assert "AMD EPYC 7763, 4 vCPU" in output


# --- (g) no control means no floor -------------------------------------------


def test__reports_without_a_control_benchmark_are_refused(tmp_path: Path):
    sides = [{TREATMENT: 0.001} for _ in range(2)]
    output = _run(
        "--group", "plain", *_write_group(tmp_path, "plain", sides), expect_ok=False
    )

    assert "::error::" in output
    assert "no floor" in output


# --- informational benchmarks are excluded, as in the gate -------------------


def test__informational_benchmarks_are_never_classified_sensitive(tmp_path: Path):
    sides = [
        {TREATMENT: 0.001, CONTROL: 0.01, INFORMATIONAL: 0.002},
        {TREATMENT: 0.001, CONTROL: 0.01, INFORMATIONAL: 0.004},  # +100%
    ]
    output = _run("--group", "plain", *_write_group(tmp_path, "plain", sides))

    row = _row(output, INFORMATIONAL)
    assert "+100.0%" in row
    assert "informational" in row
    assert "layout-sensitive" not in row


def test__a_misnamed_report_is_refused(tmp_path: Path):
    path = tmp_path / "plain.json"
    _report(path, {TREATMENT: 0.001, CONTROL: 0.01})

    output = _run("--group", "plain", str(path), expect_ok=False)

    assert "expected <group>-<salt>-<round>.json" in output
