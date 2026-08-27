"""Benchmarks over tests/data/large_message.eml.

Two of these tests are the CI performance gate, and two exist for the published
comparison table. They are kept separate on purpose, because they do not measure
the same thing.

**The gate pair** -- `test__fast_mail_parser___parse_message` and
`test__mail_parser___parse_message` -- is deliberately unchanged. The gate
compares a revision against its base, so its value comes from being stable over
time, not from being a fair cross-library comparison. Note what the baseline
actually measures: `MailParser.from_string` only calls
`email.message_from_string` and constructs the wrapper. It never calls
`.parse()`, so mail-parser's own logic is not exercised at all.

**The table pair** -- `test__mailparser_lib___full_read` and
`test__stdlib_email___full_read` -- asks the other libraries for the same
*result* fast_mail_parser produces: subject, both body lists, and attachments
with their payloads decoded. That is the comparison worth publishing, and it is
what `make bench-table` renders.

Names matter here: `.github/scripts/check_benchmark.py` selects the gate pair by
**exact** name -- substring matching was ambiguous, since any benchmark
mentioning a library name made the selection two-valued and failed the gate
rather than being ignored. New benchmarks are therefore free to be named after
what they measure; they ride along in the interleaved comparison without
disturbing the gate pair.
"""

import email
import email.policy
from collections.abc import Callable


def test__mail_parser___parse_message(large_message: str, benchmark: Callable):
    from mailparser import MailParser

    benchmark(MailParser.from_string, large_message)


def test__fast_mail_parser___parse_message(large_message: str, benchmark: Callable):
    from fast_mail_parser import PyMail, parse_email

    # Assert correctness once, outside the timed loop, so a fast-but-wrong parser
    # fails this benchmark instead of silently posting a great time. The timing
    # call below stays the sole thing `benchmark` measures.
    mail = parse_email(large_message)
    assert isinstance(mail, PyMail)
    assert mail.subject, "expected a non-empty subject from the large message"
    assert mail.headers, "expected the large message to expose headers"

    benchmark(parse_email, large_message)


def test__fast_mail_parser___parse_message_strict(large_message: str, benchmark: Callable):
    """`strict=True` on a clean message, next to the gate pair that omits it.

    #100 asks for the warning channel's overhead on the clean corpus. Strict mode
    is where it would show if it existed anywhere: the collection is the same
    work either way, and strict adds one emptiness check on a `Vec` that never
    allocated. Reading this against the benchmark above -- same round, same
    runner -- is what turns "should be free" into a number.

    It is not the gate (which selects two benchmarks by exact name) and it cannot
    be, because the base revision has no `strict` argument. The interleaved
    comparison drops a benchmark that only one side reports, so this rides along
    on the treatment side and the skip below keeps the base side green.
    """
    import pytest

    from fast_mail_parser import parse_email

    try:
        mail = parse_email(large_message, strict=True)
    except TypeError:
        pytest.skip("strict= is new in this revision; the base build has no such argument")

    # Also the assertion that matters most for the criterion: the clean corpus
    # must pass strict mode, or the benchmark would be measuring an exception.
    assert mail.warnings == [], "the benchmark message must warn about nothing"

    benchmark(parse_email, large_message, strict=True)


# --- comparison table: equivalent work across libraries ---------------------


def _fast_mail_parser_full(raw: bytes):
    from fast_mail_parser import parse_email

    mail = parse_email(raw)
    return (
        mail.subject,
        mail.text_plain,
        mail.text_html,
        [(a.mimetype, a.filename, a.content) for a in mail.attachments],
    )


def _mailparser_full(raw: str):
    from mailparser import MailParser

    parsed = MailParser.from_string(raw)
    parsed.parse()
    return (
        parsed.subject,
        parsed.text_plain,
        parsed.text_html,
        parsed.attachments,
    )


def _stdlib_full(raw: bytes):
    message = email.message_from_bytes(raw, policy=email.policy.default)
    plain, html, attachments = [], [], []
    for part in message.walk():
        if part.get_content_maintype() == "multipart":
            continue
        content_type = part.get_content_type()
        if (
            part.get_content_disposition() != "attachment"
            and content_type in ("text/plain", "text/html")
        ):
            (plain if content_type == "text/plain" else html).append(
                part.get_content()
            )
        else:
            attachments.append(
                (
                    content_type,
                    part.get_filename() or "",
                    part.get_payload(decode=True) or b"",
                )
            )
    return str(message["Subject"] or ""), plain, html, attachments


def test__fast_mail_parser___full_read(large_message: str, benchmark: Callable):
    raw = large_message.encode("utf-8", "surrogateescape")
    assert _fast_mail_parser_full(raw)[0], "expected a subject"

    benchmark(_fast_mail_parser_full, raw)


def test__mailparser_lib___full_read(large_message: str, benchmark: Callable):
    assert _mailparser_full(large_message)[0], "expected a subject"

    benchmark(_mailparser_full, large_message)


def test__stdlib_email___full_read(large_message: str, benchmark: Callable):
    raw = large_message.encode("utf-8", "surrogateescape")
    assert _stdlib_full(raw)[0], "expected a subject"

    benchmark(_stdlib_full, raw)


def test__fast_mail_parser___parse_many(large_message: str, benchmark: Callable):
    from fast_mail_parser import parse_many

    # Single-threaded on purpose: this measures per-payload overhead -- the
    # handling of the caller's bytes -- and thread scheduling would only add
    # noise to that. Parallel throughput is a separate question.
    batch = [large_message.encode()] * 8

    benchmark(lambda: parse_many(batch, threads=1))


# --- parse_many against Python-side threading -------------------------------
#
# #96's acceptance criteria ask for this comparison. Both are `test__threaded___`
# and both are informational: the gate reports them and does not judge them,
# because thread scheduling moves them more than a code change would, so gating
# on them would buy flakes rather than protection.
#
# Note what is NOT being compared. `parse_email` has released the GIL since #91,
# so the thread pool below gets real parallelism too -- this is not
# parallel-versus-serial. What is left is per-call overhead: one FFI crossing for
# the batch against one per message, plus Python's own thread and future
# machinery.

BATCH = 16


def test__threaded___parse_many(large_message: str, benchmark: Callable):
    from fast_mail_parser import parse_many

    batch = [large_message.encode()] * BATCH

    benchmark(lambda: parse_many(batch))


def test__threaded___threadpool_parse_email(large_message: str, benchmark: Callable):
    import os
    from concurrent.futures import ThreadPoolExecutor

    from fast_mail_parser import parse_email

    batch = [large_message.encode()] * BATCH

    # The pool is built outside the timed call. A pipeline reuses one; charging
    # thread creation to every batch would flatter parse_many for the wrong
    # reason.
    with ThreadPoolExecutor(max_workers=os.cpu_count()) as pool:
        benchmark(lambda: list(pool.map(parse_email, batch)))


# Message size decides this comparison, so measure both ends of it. Above, one
# batch of 16 x 0.75 MiB: parsing dominates and per-call overhead is invisible.
# Here, 2000 x ~0.8 KiB, where the opposite holds and the per-message cost of
# crossing into Rust and back -- plus a Python future per message -- is the whole
# difference.
SMALL_BATCH = 2000


def _small_message() -> bytes:
    body = "x" * 700
    return (
        "From: sender@example.com\r\n"
        "To: recipient@example.com\r\n"
        "Subject: small\r\n"
        "Content-Type: text/plain; charset=utf-8\r\n"
        "\r\n"
        f"{body}\r\n"
    ).encode()


def test__threaded___parse_many_small(benchmark: Callable):
    from fast_mail_parser import parse_many

    batch = [_small_message()] * SMALL_BATCH

    benchmark(lambda: parse_many(batch))


def test__threaded___threadpool_parse_email_small(benchmark: Callable):
    import os
    from concurrent.futures import ThreadPoolExecutor

    from fast_mail_parser import parse_email

    batch = [_small_message()] * SMALL_BATCH

    with ThreadPoolExecutor(max_workers=os.cpu_count()) as pool:
        benchmark(lambda: list(pool.map(parse_email, batch)))


def test__fast_mail_parser___parse_tree(large_message: str, benchmark: Callable):
    # The structural API against the flat one, on the same message. #99 asks for
    # the overhead to be measured rather than assumed, and the tree genuinely does
    # more: it decodes every leaf, including parts the flat projection drops, and
    # builds a Python object per part. Compare with
    # `test__fast_mail_parser___parse_message` in the same run.
    from fast_mail_parser import parse_email_tree

    payload = large_message.encode()

    benchmark(lambda: parse_email_tree(payload))


def test__fast_mail_parser___parse_metadata(large_message: str, benchmark: Callable):
    # #97 asks for metadata mode to be at least 5x faster than a full parse on an
    # attachment-heavy message. Compare with
    # `test__fast_mail_parser___parse_message` in the same run.
    #
    # Guarded, and this is the guard the gate's own documentation asks for: the
    # gate measures THIS revision's benchmarks against the BASE revision's build
    # (#168), so a base predating `mode=` raises TypeError here instead of
    # skipping, and takes the whole gate down with it. Which is what happened.
    import pytest

    from fast_mail_parser import parse_email

    payload = large_message.encode()

    try:
        parse_email(payload, mode="metadata")
    except TypeError:
        pytest.skip("this build predates parse_email(mode=...)")

    benchmark(lambda: parse_email(payload, mode="metadata"))


def test__fast_mail_parser___parse_lazy_untouched(large_message: str, benchmark: Callable):
    # #97's lazy mode with nothing read: the parse decodes the bodies and defers
    # every attachment, so on this fixture -- 99% attachment by decoded content --
    # this should land near `test__fast_mail_parser___parse_metadata` and far below
    # `test__fast_mail_parser___parse_message`. Compare all three in the same run.
    #
    # Guarded like the metadata benchmark, and against `ValueError` as well as
    # `TypeError`: the gate measures THIS revision's benchmarks against the BASE
    # revision's build (#168), and a base that already has `mode=` rejects an
    # unknown mode with `ValueError` rather than failing to accept the argument.
    import pytest

    from fast_mail_parser import parse_email

    payload = large_message.encode()

    try:
        parse_email(payload, mode="lazy")
    except (TypeError, ValueError):
        pytest.skip('this build predates parse_email(mode="lazy")')

    benchmark(lambda: parse_email(payload, mode="lazy"))


def test__fast_mail_parser___parse_lazy_all_attachments(large_message: str, benchmark: Callable):
    # The other end of the trade, measured rather than asserted: lazy mode plus
    # reading every attachment does the full parse's work in a worse order --
    # a copy of each part's encoded bytes, then a re-parse of its headers per
    # attachment. Whoever is going to decode everything anyway should use the
    # default mode, and this is the number that says so.
    import pytest

    from fast_mail_parser import parse_email

    payload = large_message.encode()

    try:
        parse_email(payload, mode="lazy")
    except (TypeError, ValueError):
        pytest.skip('this build predates parse_email(mode="lazy")')

    def read_everything():
        mail = parse_email(payload, mode="lazy")
        return [len(attachment.content) for attachment in mail.attachments]

    benchmark(read_everything)


def test__fast_mail_parser___parse_tree_metadata(large_message: str, benchmark: Callable):
    # #202's tree metadata mode against `test__fast_mail_parser___parse_tree` in
    # the same run: the same walk, the same node per part, and no leaf decoded.
    # On this fixture -- 99% attachment by decoded content -- that is nearly all
    # of the tree's work.
    #
    # Guarded, as the gate's documentation requires: the gate measures THIS
    # revision's benchmarks against the BASE revision's build (#168), so a base
    # predating this argument raises TypeError here instead of skipping and takes
    # the whole gate down with it.
    import pytest

    from fast_mail_parser import parse_email_tree

    payload = large_message.encode()

    try:
        parse_email_tree(payload, mode="metadata")
    except (TypeError, ValueError):
        pytest.skip('this build predates parse_email_tree(mode="metadata")')

    benchmark(lambda: parse_email_tree(payload, mode="metadata"))


def test__fast_mail_parser___parse_tree_lazy_untouched(large_message: str, benchmark: Callable):
    # The other deferred tree mode with nothing read. It retains a copy of every
    # leaf where metadata mode retains nothing, so the gap between this and the
    # benchmark above is the price of being able to decode one part later.
    import pytest

    from fast_mail_parser import parse_email_tree

    payload = large_message.encode()

    try:
        parse_email_tree(payload, mode="lazy")
    except (TypeError, ValueError):
        pytest.skip('this build predates parse_email_tree(mode="lazy")')

    benchmark(lambda: parse_email_tree(payload, mode="lazy"))


def test__fast_mail_parser___parse_many_metadata(large_message: str, benchmark: Callable):
    # #202's headline case: the batch API and metadata mode composed. Compare with
    # `test__fast_mail_parser___parse_many` in the same run -- same batch, same
    # single worker, so the difference is the mode and nothing else.
    import pytest

    from fast_mail_parser import parse_many

    batch = [large_message.encode()] * 8

    try:
        parse_many(batch[:1], mode="metadata")
    except (TypeError, ValueError):
        pytest.skip('this build predates parse_many(mode="metadata")')

    benchmark(lambda: parse_many(batch, threads=1, mode="metadata"))


def test__threaded___parse_many_metadata_small(benchmark: Callable):
    # The mailbox sweep the mode was built for, at the size where the batch API's
    # own saving lives (#96: 2000 x ~0.8 KiB). Informational, like its siblings:
    # thread scheduling moves these more than a code change would.
    import pytest

    from fast_mail_parser import parse_many

    batch = [_small_message()] * SMALL_BATCH

    try:
        parse_many(batch[:1], mode="metadata")
    except (TypeError, ValueError):
        pytest.skip('this build predates parse_many(mode="metadata")')

    benchmark(lambda: parse_many(batch, mode="metadata"))
