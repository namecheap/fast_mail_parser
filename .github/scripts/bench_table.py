#!/usr/bin/env python3
"""Render the published cross-library benchmark table from a pytest-benchmark run.

Reads the JSON report written by `pytest tests/benchmark --benchmark-json=...`
and prints a Markdown table comparing the three libraries on **equivalent
work** -- each is asked to produce the same result: subject, both body lists,
and attachments with payloads decoded.

Only the `*___full_read` benchmarks are published. The two gate benchmarks are
excluded on purpose: `test__mail_parser___parse_message` measures
`MailParser.from_string`, which never calls `.parse()`, so it is not a fair
cross-library figure. It exists to be stable for the regression gate, not to be
quoted.

Usage:
    python bench_table.py [benchmark.json]
"""

import importlib.metadata
import json
import platform
import sys

# benchmark name -> (display label, what the timed call actually does)
ROWS = [
    (
        "test__fast_mail_parser___full_read",
        "**fast_mail_parser**",
        "parse + decode bodies + decode attachments",
    ),
    (
        "test__mailparser_lib___full_read",
        "mail-parser",
        "`from_string` + `.parse()` + read attributes",
    ),
    (
        "test__stdlib_email___full_read",
        "stdlib `email`",
        "`message_from_bytes` + walk + `get_content` / `get_payload`",
    ),
]


def _version(dist: str) -> str:
    try:
        return importlib.metadata.version(dist)
    except importlib.metadata.PackageNotFoundError:
        return "unknown"


def main() -> int:
    path = sys.argv[1] if len(sys.argv) > 1 else "benchmark.json"
    with open(path) as handle:
        report = json.load(handle)

    mins = {b["name"]: b["stats"]["min"] for b in report["benchmarks"]}

    missing = [name for name, _, _ in ROWS if name not in mins]
    if missing:
        sys.exit(f"benchmark(s) missing from {path}: {', '.join(missing)}")

    fastest = mins[ROWS[0][0]]

    print("| Library | Work performed | Min time | Relative |")
    print("| --- | --- | --- | --- |")
    for name, label, work in ROWS:
        ms = mins[name] * 1e3
        print(f"| {label} | {work} | {ms:.2f} ms | {ms / (fastest * 1e3):.2f}x |")

    machine = report.get("machine_info", {})
    print()
    print(
        f"Corpus: `tests/data/large_message.eml` "
        f"(multipart/mixed, 6 MIME parts, 2 base64 attachments). "
        f"CPython {machine.get('python_version', platform.python_version())} "
        f"on {machine.get('system', platform.system())} "
        f"{machine.get('machine', platform.machine())}. "
        f"fast-mail-parser {_version('fast-mail-parser')}, "
        f"mail-parser {_version('mail-parser')}. "
        f"Minimum of {report['benchmarks'][0]['stats']['rounds']}+ rounds."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
