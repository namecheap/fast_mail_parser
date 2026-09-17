"""Peak memory of a payload handed to the parser (#96).

Every payload used to be copied into Rust-owned memory before any parsing began,
so a batch cost its own size again in duplicates while the caller still held the
originals. `bytes` stopped being copied in #96 and `str` in #226; both are now
borrowed straight out of the Python object.

Two things make this measurable at all:

**An oversized payload.** A payload just over MAX_INPUT_BYTES is rejected before
any parsing work (see `test_dos_limits.py`), so nothing else allocates and the
copy is essentially the only memory movement left. For a payload that parses
normally, the decoded result would dominate and hide it.

**A subprocess.** `ru_maxrss` is a high-water mark that never falls, so any
earlier test in this process that allocated more would leave `before` already
above what the call reaches — and the assertion would pass without measuring
anything. It runs outside the repository root, so that the source package there
does not shadow the installed extension.
"""
import subprocess
import sys
import textwrap

import pytest

pytestmark = pytest.mark.skipif(
    not sys.platform.startswith("linux"),
    reason="ru_maxrss is kilobytes on Linux and bytes on macOS; keep the units unambiguous",
)

# Copying the payload would cost this much; the allowance is a fifth of it.
PAYLOAD_MIB = 100
ALLOWED_GROWTH_MIB = PAYLOAD_MIB / 5

PROBE = textwrap.dedent(
    """
    import resource
    import sys

    from fast_mail_parser import MimeStructureError, parse_email

    limit = 100 * 1024 * 1024
    payload = b"Subject: big\\r\\n\\r\\n" + b"x" * (limit + 1)

    # After building the payload: its own cost is part of the baseline, so what
    # is measured is only what the parser adds on top.
    before = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    try:
        parse_email(payload)
    except MimeStructureError:
        pass
    else:
        sys.exit("the oversized payload was not rejected")
    after = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss

    print((after - before) / 1024)
    """
)

# The `str` twin (#226). Same shape, same allowance: a `str` payload is borrowed
# through `PyBackedStr`, so rejecting an oversized one must not cost a copy of it
# either. The literal is pure ASCII, which is the case where CPython has the UTF-8
# form already and hands over its internal buffer with no encoding step.
PROBE_STR = textwrap.dedent(
    """
    import resource
    import sys

    from fast_mail_parser import MimeStructureError, parse_email

    limit = 100 * 1024 * 1024
    payload = "Subject: big\\r\\n\\r\\n" + "x" * (limit + 1)

    before = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    try:
        parse_email(payload)
    except MimeStructureError:
        pass
    else:
        sys.exit("the oversized payload was not rejected")
    after = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss

    print((after - before) / 1024)
    """
)


def test__an_oversized_payload_is_not_copied_before_being_rejected(tmp_path):
    # Run it from somewhere other than the repository root. `python -c` puts the
    # working directory first on sys.path, so from the root the source
    # `fast_mail_parser/` package shadows the installed wheel and the probe
    # imports a package with no compiled extension in it.
    probe = subprocess.run(
        [sys.executable, "-c", PROBE],
        capture_output=True,
        text=True,
        cwd=tmp_path,
    )

    assert probe.returncode == 0, probe.stderr
    growth = float(probe.stdout.strip())

    assert growth < ALLOWED_GROWTH_MIB, (
        f"peak memory grew {growth:.1f} MiB while rejecting a {PAYLOAD_MIB} MiB "
        f"payload that is discarded before parsing; more than "
        f"{ALLOWED_GROWTH_MIB:.0f} MiB suggests it is being copied first"
    )


def test__an_oversized_str_payload_is_not_copied_before_being_rejected(tmp_path):
    # From `tmp_path` for the same reason as the `bytes` probe above.
    probe = subprocess.run(
        [sys.executable, "-c", PROBE_STR],
        capture_output=True,
        text=True,
        cwd=tmp_path,
    )

    assert probe.returncode == 0, probe.stderr
    growth = float(probe.stdout.strip())

    assert growth < ALLOWED_GROWTH_MIB, (
        f"peak memory grew {growth:.1f} MiB while rejecting a {PAYLOAD_MIB} MiB "
        f"`str` payload that is discarded before parsing; more than "
        f"{ALLOWED_GROWTH_MIB:.0f} MiB suggests it is being copied first"
    )


def test__bytes_and_str_payloads_agree(valid_message: str):
    # Both paths borrow, but through different handles (`PyBackedStr` vs
    # `PyBackedBytes`), so they differ internally and must not differ in result.
    from fast_mail_parser import parse_many

    from_str, from_bytes = parse_many([valid_message, valid_message.encode()])

    assert from_str.subject == from_bytes.subject
    assert from_str.text_plain == from_bytes.text_plain
    assert list(from_str.headers) == list(from_bytes.headers)
