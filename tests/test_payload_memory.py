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

The lazy probe at the end of this file measures a payload that parses, and so
needs a third thing: a payload built on **disk** rather than in memory. Building
a 96 MiB payload in the probe means holding a temporary and the result at once,
which raises the water mark to twice the payload before the parser is reached —
and a parser that copied the whole thing would then fit underneath it and measure
as free. Reading it from a file is one allocation, so the mark starts at one
payload and a copy is visible as a second.
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


# --- what a lazy parse retains (#239) -----------------------------------------

# Lazy mode used to copy each deferred part out of the message, so a message that
# is one large attachment cost its own size again the moment it was parsed —
# while deferring the decode, which is the cost the mode exists to avoid. It now
# keeps offsets and pins the payload.
LAZY_PAYLOAD_MIB = 96

PROBE_LAZY = textwrap.dedent(
    """
    import resource
    import sys

    from fast_mail_parser import parse_email

    # Before the payload exists: what is measured is the payload plus whatever
    # the parse adds, so a retained copy of the attachment shows up as a second
    # payload rather than disappearing under the first one's water mark.
    before = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    with open(sys.argv[1], "rb") as handle:
        payload = handle.read()

    mail = parse_email(payload, mode="lazy")
    after = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss

    if len(mail.attachments) != 1:
        sys.exit(f"expected one attachment, got {len(mail.attachments)}")
    if mail.attachments[0].is_decoded:
        sys.exit("the attachment was decoded by the parse")

    print((after - before) / 1024)
    """
)


def _write_one_big_attachment(path, mib: int) -> None:
    # Written in chunks so this process never holds the payload either. It is not
    # measured here, but a 96 MiB temporary in the test runner is worth avoiding
    # on its own.
    with open(path, "wb") as handle:
        handle.write(
            b"Subject: one big attachment\r\n"
            b'Content-Type: multipart/mixed; boundary="b"\r\n'
            b"\r\n"
            b"--b\r\n"
            b"Content-Type: text/plain\r\n"
            b"\r\n"
            b"hello\r\n"
            b"--b\r\n"
            b"Content-Type: application/octet-stream\r\n"
            b"Content-Disposition: attachment; filename=big.bin\r\n"
            b"Content-Transfer-Encoding: base64\r\n"
            b"\r\n"
        )
        chunk = b"A" * 1024 * 1024
        for _ in range(mib):
            handle.write(chunk)
        handle.write(b"\r\n--b--\r\n")


def test__a_lazy_parse_does_not_copy_the_attachment_it_defers(tmp_path):
    message = tmp_path / "big.eml"
    _write_one_big_attachment(message, LAZY_PAYLOAD_MIB)

    probe = subprocess.run(
        [sys.executable, "-c", PROBE_LAZY, str(message)],
        capture_output=True,
        text=True,
        cwd=tmp_path,
    )

    assert probe.returncode == 0, probe.stderr
    growth = float(probe.stdout.strip())

    # One payload, plus the allowance. Two payloads is what a retained copy costs
    # and is the failure this is looking for, so the allowance has room to spare
    # without letting that through.
    allowed = LAZY_PAYLOAD_MIB + ALLOWED_GROWTH_MIB
    assert growth < allowed, (
        f"peak memory grew {growth:.1f} MiB while parsing a {LAZY_PAYLOAD_MIB} MiB "
        f"message whose attachment was never read; more than {allowed:.0f} MiB "
        f"means the deferred part was copied rather than pointed at"
    )
