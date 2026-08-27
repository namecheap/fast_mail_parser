"""Peak memory of a payload handed to the parser (#96).

Every payload used to be copied into Rust-owned memory before any parsing began,
so a batch cost its own size again in duplicates while the caller still held the
originals. Payloads that are `bytes` are now borrowed.

Two things make this measurable at all:

**An oversized payload.** A payload just over MAX_INPUT_BYTES is rejected before
any parsing work (see `test_dos_limits.py`), so nothing else allocates and the
copy is essentially the only memory movement left. For a payload that parses
normally, the decoded result would dominate and hide it.

**A subprocess.** `ru_maxrss` is a high-water mark that never falls, so any
earlier test in this process that allocated more would leave `before` already
above what the call reaches — and the assertion would pass without measuring
anything.
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


def test__an_oversized_payload_is_not_copied_before_being_rejected():
    probe = subprocess.run(
        [sys.executable, "-c", PROBE], capture_output=True, text=True
    )

    assert probe.returncode == 0, probe.stderr
    growth = float(probe.stdout.strip())

    assert growth < ALLOWED_GROWTH_MIB, (
        f"peak memory grew {growth:.1f} MiB while rejecting a {PAYLOAD_MIB} MiB "
        f"payload that is discarded before parsing; more than "
        f"{ALLOWED_GROWTH_MIB:.0f} MiB suggests it is being copied first"
    )


def test__bytes_and_str_payloads_agree(valid_message: str):
    # `str` still gets copied -- the limited API has no UTF-8 buffer to borrow --
    # so the two paths differ internally and must not differ in result.
    from fast_mail_parser import parse_many

    from_str, from_bytes = parse_many([valid_message, valid_message.encode()])

    assert from_str.subject == from_bytes.subject
    assert from_str.text_plain == from_bytes.text_plain
    assert list(from_str.headers) == list(from_bytes.headers)
