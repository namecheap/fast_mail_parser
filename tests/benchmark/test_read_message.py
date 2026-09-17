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

import base64
import email
import email.policy
from collections.abc import Callable

# Module level, unlike the function-local imports elsewhere in this file: the
# parametrised benchmarks below need `pytest.param` at decoration time.
import pytest


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


def test__fast_mail_parser___attachment_reread(large_message: str, benchmark: Callable):
    """Re-reading attachments off an already-parsed message (#227).

    The parse is done once, outside the timed body, so what is left is only what
    a consumer pays to *get at* the bytes: one `mail.attachments` read and one
    `.content` read per attachment. That is the library's own documented idiom
    (README, `by_cid = {a.content_id: a for a in mail.attachments}`), and before
    #227 it was two full copies of every attachment's decoded payload -- on this
    fixture, 99% attachment by decoded content, that is most of a megabyte per
    iteration for work that produces nothing new.

    Not guarded: it uses no argument the base build lacks.
    """
    from fast_mail_parser import parse_email

    mail = parse_email(large_message.encode())
    assert mail.attachments, "expected the large message to carry attachments"

    benchmark(lambda: [a.content for a in mail.attachments])


def test__fast_mail_parser___parse_qp_message(valid_message: str, benchmark: Callable):
    """The only fixture whose bodies are quoted-printable (#229).

    89,932 bytes of text/html and 8,122 of text/plain, bare-LF on the wire. Every
    other benchmark in this file is base64 or 8bit, so this is the one that sees
    the quoted-printable decoder at all -- which is why the maintainer's profile
    of a full parse describes base64 mail only, and why the gate could not have
    noticed this path getting slower.
    """
    from fast_mail_parser import parse_email

    payload = valid_message.encode()

    # Asserted once, outside the timed call, so a fast-but-wrong decoder fails
    # here instead of posting a good time.
    mail = parse_email(payload)
    assert mail.text_html and mail.text_plain, "both QP parts must decode"
    assert "=3D" not in mail.text_html[0], "the HTML part must actually be decoded"
    assert "\r\n" in mail.text_html[0], "robust decoding canonicalises bare LF to CRLF"

    benchmark(parse_email, payload)


def test__fast_mail_parser___parse_qp_message_metadata(valid_message: str, benchmark: Callable):
    """The non-decoding control on the same fixture (#229).

    Metadata mode never transfer-decodes, so this pays the header and structure
    cost of `valid_message` and nothing else. The gap between it and the
    benchmark above is the quoted-printable decode.
    """
    import pytest

    from fast_mail_parser import parse_email

    payload = valid_message.encode()

    try:
        parse_email(payload, mode="metadata")
    except TypeError:
        pytest.skip("this build predates parse_email(mode=...)")

    benchmark(lambda: parse_email(payload, mode="metadata"))


def test__fast_mail_parser___headers_first_read(large_message: str, benchmark: Callable):
    """Parse in metadata mode and read `headers` once (#231).

    The cost of building the dict has not gone away, it has moved to first
    access -- so this pays exactly one build either way and is the control that
    says caching did not make the first read more expensive.
    """
    import pytest

    from fast_mail_parser import parse_email

    payload = large_message.encode()

    try:
        parse_email(payload, mode="metadata")
    except TypeError:
        pytest.skip("this build predates parse_email(mode=...)")

    benchmark(lambda: parse_email(payload, mode="metadata").headers)


def test__fast_mail_parser___headers_repeat_read(large_message: str, benchmark: Callable):
    """Three header lookups on an already-parsed message (#231).

    The parse is outside the timed call, so this is purely what a caller pays to
    *read* headers -- the README's own idiom, `h.get("From")` then
    `h.get("Subject")` then a `Received` sweep. Before #231 each of those rebuilt
    the whole dict; now they are three dict lookups on one shared object.
    """
    from fast_mail_parser import parse_email

    mail = parse_email(large_message.encode())
    assert mail.headers, "expected the large message to expose headers"

    benchmark(
        lambda: (
            mail.headers.get("From"),
            mail.headers.get("Subject"),
            mail.headers.get("Received"),
        )
    )


# --- message shapes the gate could not see (#223) -----------------------------
#
# Every gated benchmark measured one message: `large_message.eml`, 767 KiB and
# 99% base64 attachment. So the gate judged the decode path and nothing else, and
# a change that halved header handling or doubled the per-call floor would have
# passed it unnoticed -- which is not hypothetical: #238's header work moved the
# small serial batch 23% while `parse_message` moved 2%.
#
# These are gated rather than informational on purpose: coverage of the judged
# set is the whole point. They use only APIs the comparison base has, so no skip
# guard is needed.


def _rfc2047_heavy_message() -> bytes:
    """A header-heavy message with encoded words throughout (~30 KB).

    Built here rather than committed under `tests/data/`: every `.eml` there is
    auto-enrolled in eight correctness suites (the RFC corpus requires a `CASES`
    entry, packaging invariants require a `BUILDERS` entry, and the parity, lazy,
    metadata, tree and warning suites all glob the directory). A benchmark input
    has no business being an oracle. `_small_message()` is the precedent.

    Deterministic, so the benchmark measures the same bytes on every run.
    """
    encoded = "=?utf-8?B?" + base64.b64encode("Café ☕ déjà vu ".encode()).decode() + "?="
    lines = [
        "From: " + encoded + " <sender@example.com>",
        "To: " + ", ".join(f"{encoded} <r{i}@example.com>" for i in range(5)),
        "Subject: " + " ".join([encoded] * 6),
        "Date: Mon, 01 Jan 2024 12:00:00 +0000",
        "Message-ID: <bench@fast-mail-parser.test>",
    ]
    lines += [
        f"Received: from mx{i}.example.net by mx{i + 1}.example.net with ESMTP "
        f"id ABC{i:04d};\r\n\tMon, 01 Jan 2024 12:00:{i % 60:02d} +0000"
        for i in range(40)
    ]
    lines += [
        f"X-Header-{i}: value-{i} {'=?utf-8?Q?caf=C3=A9?=' if i % 10 == 0 else ''}"
        for i in range(200)
    ]
    lines += ["Content-Type: text/plain; charset=utf-8", "", "body", ""]
    return "\r\n".join(lines).encode()


def test__fast_mail_parser___parse_small(benchmark: Callable):
    """The per-call floor: FFI, header map, address and date parse on ~0.8 KB.

    The gate's own message is 767 KiB, so nothing in the judged set could see a
    change to fixed per-call cost. This is the benchmark where that shows.
    """
    from fast_mail_parser import parse_email

    payload = _small_message()

    mail = parse_email(payload)
    assert mail.subject == "small"
    assert mail.headers, "expected headers"
    assert mail.warnings == [], "a benchmark input must not be timing a repair"

def test__fast_mail_parser___parse_8bit_text(benchmark: Callable):
    """A plain 8bit text body, which is where the removed copy shows (#230).

    7bit/8bit/binary bodies *are* their raw bytes, so `get_body_raw` used to copy
    them into a `Vec` purely so the charset step could borrow them again. No
    transfer decoding happens here at all, so this benchmark is almost entirely
    that copy plus the charset validation.
    """
    from fast_mail_parser import parse_email

    # ~128 KB. Sized so the body handling dominates rather than the fixed
    # per-call cost: at 400 repetitions this ran in 6 us on the CI runner, where
    # FFI and the header parse are most of it and a 2 us wobble reads as 30%.
    body = ("Wir müssen die Nachricht lesen. " * 4000).encode()
    payload = (
        b"Subject: eight bit\r\n"
        b"Content-Type: text/plain; charset=utf-8\r\n"
        b"Content-Transfer-Encoding: 8bit\r\n\r\n" + body + b"\r\n"
    )

    mail = parse_email(payload)
    assert mail.text_plain and "müssen" in mail.text_plain[0]
    assert mail.warnings == []

    benchmark(parse_email, payload)


def test__fast_mail_parser___parse_many_small_serial(benchmark: Callable):
    """The same per-call cost x2000, serial, so no scheduling noise rides along.

    The gated form to prefer over the single-call floor above if that one proves
    too noisy on the runner: same path, milliseconds instead of microseconds.
    """
    from fast_mail_parser import parse_many

    batch = [_small_message()] * SMALL_BATCH

    assert len(parse_many(batch[:1], threads=1)) == 1

    benchmark(lambda: parse_many(batch, threads=1))


def test__fast_mail_parser___parse_rfc2047_headers(benchmark: Callable):
    """Encoded-word decoding and a large header block -- the one path no other
    gated benchmark touches.

    `valid_message.eml` covers quoted-printable bodies and a realistic 30-header
    block; what it has almost none of is RFC 2047. Here the tokenizer runs on
    every one of ~250 headers and actually decodes on ~30 of them.
    """
    from fast_mail_parser import parse_email

    payload = _rfc2047_heavy_message()

    mail = parse_email(payload)
    assert mail.subject.startswith("Café"), "encoded words must decode"
    assert len(mail.headers["Received"]) == 40
    assert mail.warnings == [], "a benchmark input must not be timing a repair"

def test__fast_mail_parser___parse_base64_utf8_text(benchmark: Callable):
    """A base64 text body labelled UTF-8, the owned-decode path (#230).

    The transfer decode allocates a `Vec`; handing that to `String::from_utf8`
    instead of lending it to encoding_rs and copying the result is the second
    copy this removes.
    """
    import base64

    from fast_mail_parser import parse_email

    # ~128 KB, for the reason given on the 8bit benchmark above: at 400
    # repetitions this was 11 us on the CI runner and its gate verdict swung
    # 14.5% on a 2 us difference.
    body = ("Wir müssen die Nachricht lesen. " * 4000).encode()
    payload = (
        b"Subject: base64 text\r\n"
        b"Content-Type: text/plain; charset=utf-8\r\n"
        b"Content-Transfer-Encoding: base64\r\n\r\n" + base64.b64encode(body) + b"\r\n"
    )

    mail = parse_email(payload)
    assert mail.text_plain and "müssen" in mail.text_plain[0]
    assert mail.warnings == []

    benchmark(parse_email, payload)


def test__fast_mail_parser___parse_qp_dense_escapes(benchmark: Callable):
    """A quoted-printable body that is mostly escapes (#230).

    The escape scan runs over every quoted-printable body, and its old form was a
    byte-at-a-time loop. This is its worst case: one `=` every three bytes, so
    `memchr` has to find nearly all of them and the predicate runs nearly every
    time -- if the scan were slower anywhere, it would be here.
    """
    from fast_mail_parser import parse_email

    payload = (
        b"Subject: dense\r\n"
        b"Content-Type: text/plain; charset=utf-8\r\n"
        b"Content-Transfer-Encoding: quoted-printable\r\n\r\n"
        + b"=C3=A9" * 20000
        + b"\r\n"
    )

    mail = parse_email(payload)
    assert mail.text_plain and mail.text_plain[0].startswith("é")
    assert mail.warnings == []

    benchmark(parse_email, payload)


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

def _distinct(message: bytes, count: int) -> list[bytes]:
    """`count` separate buffers with identical content (#224).

    `[message] * count` is `count` references to *one* object, and since #96
    `parse_many` borrows `bytes` rather than copying them -- so every worker
    would be reading the same cache-hot buffer. A mailbox is `count` distinct
    buffers, and for the 2000-message batch that is 1.6 MB spread over L2/L3
    instead of one 800-byte line resident in L1. Aliasing flatters the parallel
    path in exactly the benchmark that publishes the batch API's headline.

    `bytes(b)` and `b[:]` return the same object for `bytes` in CPython, so the
    copy has to go through `bytearray`; the assertion is there because a future
    CPython optimisation that made it a no-op would silently restore the
    aliasing this exists to avoid.
    """
    batch = [bytes(bytearray(message)) for _ in range(count)]
    assert len({id(payload) for payload in batch}) == count, "batch slots alias"
    return batch


BATCH = 16


def test__threaded___parse_many(large_message: str, benchmark: Callable):
    from fast_mail_parser import parse_many

    batch = _distinct(large_message.encode(), BATCH)

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


def test__threaded___parse_many_metadata_small_threads1(benchmark: Callable):
    """The same batch, serial, in metadata mode (#238).

    Its full-mode counterpart is `test__fast_mail_parser___parse_many_small_serial`,
    which is gated (#223); this one stays informational because metadata mode has
    no gated benchmark on this batch to be read against.

    Metadata mode transfer-decodes nothing, so on a small message this is very
    nearly pure header work -- the narrowest read available on that path.
    """
    import pytest

    from fast_mail_parser import parse_many

    batch = [_small_message()] * SMALL_BATCH

    try:
        parse_many(batch[:1], mode="metadata")
    except TypeError:
        pytest.skip('this build predates parse_many(mode="metadata")')

    benchmark(lambda: parse_many(batch, threads=1, mode="metadata"))


# An IMAP fetch page or a queue poll: a handful of small messages, which is the
# shape a mail pipeline produces most often and the one with no benchmark before
# #232. Well under MIN_BYTES_PER_WORKER, so with the default thread count this
# batch now parses on the calling thread instead of spawning one worker per
# message to parse about a microsecond each.
SMALL_PAGE = 16


def test__threaded___parse_many_small_page(benchmark: Callable):
    """`parse_many` on a 16-message page with the default thread count (#232)."""
    from fast_mail_parser import parse_many

    payloads = [_small_message() for _ in range(SMALL_PAGE)]

    benchmark(lambda: parse_many(payloads))


def test__threaded___parse_many_small_page_threads1(benchmark: Callable):
    """The same page forced serial, as the reference for the one above.

    Before #232 the default-threads version was the slower of the two -- the
    scheduler cost more than the parsing it scheduled. They should now agree.
    """
    from fast_mail_parser import parse_many

    payloads = [_small_message() for _ in range(SMALL_PAGE)]

    benchmark(lambda: parse_many(payloads, threads=1))


def test__threaded___parse_many_small(benchmark: Callable):
    from fast_mail_parser import parse_many

    batch = _distinct(_small_message(), SMALL_BATCH)

    benchmark(lambda: parse_many(batch))


# --- what the batch API's shape actually costs (#224) -------------------------
#
# Three questions the suite could not answer. How does `parse_many` scale with
# `threads` -- i.e. how big is the serial section, the result marshalling that
# runs under the GIL after the parse? Does the atomic-cursor scheduler earn its
# keep on the uneven batches it was written for, when every batch measured so far
# has been homogeneous? And what does a caller pay for aliasing, now that
# payloads are borrowed rather than copied?
#
# All informational: they are here to be read, not to gate. The ids are fixed
# strings so a benchmark's name does not depend on the machine's core count.

THREAD_POINTS = [
    pytest.param(1, id="t1"),
    pytest.param(2, id="t2"),
    pytest.param(4, id="t4"),
    pytest.param(None, id="all"),
]


@pytest.mark.parametrize("threads", THREAD_POINTS)
def test__threaded___parse_many_small_scaling(benchmark: Callable, threads):
    """T(threads) on 2000 small messages.

    `[t1]` against `[all]` is the serial fraction of the small-message path read
    off directly -- the per-message marshalling under the GIL, plus the spawn.
    `[t2]` and `[t4]` say whether it flattens early, which a two-point
    measurement cannot.
    """
    from fast_mail_parser import parse_many

    batch = _distinct(_small_message(), SMALL_BATCH)

    benchmark(lambda: parse_many(batch, threads=threads))


def _mixed_batch() -> list[bytes]:
    """200 messages of wildly different sizes, in a fixed shuffled order.

    The workload the cursor scheduler exists for: the parsing core justifies
    the atomic cursor over static chunking because "static chunking would stall a
    worker that happened to draw several large messages, and real mail batches are
    very uneven in size". Every other batch here is one size repeated, so nothing
    measured that claim.

    Seeded, so the draw order is the same on every machine and run. Distinct
    buffers throughout. `invalid_message.eml` is left out on purpose -- an error
    slot is realistic, but it changes what is being measured.
    """
    import glob
    import random

    def read(path: str) -> bytes:
        with open(path, "rb") as fh:
            return fh.read()

    large = read("tests/data/large_message.eml")
    medium = read("tests/data/valid_message.eml")
    tiny = read("tests/data/attachment_message.eml")
    rfc = [read(path) for path in sorted(glob.glob("tests/data/rfc/*.eml"))]

    batch = (
        _distinct(large, 4)
        + _distinct(medium, 16)
        + _distinct(tiny, 60)
        + [bytes(bytearray(message)) for message in rfc for _ in range(8)]
    )
    random.Random(0).shuffle(batch)
    return batch


@pytest.mark.parametrize("threads", [pytest.param(1, id="t1"), pytest.param(None, id="all")])
def test__threaded___parse_many_mixed(benchmark: Callable, threads):
    """An uneven batch, which is what real mail looks like."""
    from fast_mail_parser import parse_many

    batch = _mixed_batch()

    # Asserted once, outside the timed call: a batch where some slot fails would
    # be measuring the error path and reporting it as throughput.
    assert len(parse_many(batch)) == len(batch)

    benchmark(lambda: parse_many(batch, threads=threads))


def test__threaded___parse_many_mixed_metadata(benchmark: Callable):
    """The same uneven batch in metadata mode, which never transfer-decodes.

    Against `test__threaded___parse_many_mixed[all]` this separates the scheduling
    of an uneven batch from the decoding of it -- the sizes still differ wildly,
    but the work per slot no longer does.
    """
    from fast_mail_parser import parse_many

    batch = _mixed_batch()

    try:
        parse_many(batch[:1], mode="metadata")
    except (TypeError, ValueError):
        pytest.skip('this build predates parse_many(mode="metadata")')

    benchmark(lambda: parse_many(batch, mode="metadata"))


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


def test__fast_mail_parser___parse_metadata_str(large_message: str, benchmark: Callable):
    """`mode="metadata"` fed a `str` rather than `bytes` (#226).

    The benchmark above and this one do identical parsing work on identical bytes;
    the only difference is how the payload crosses the boundary. Metadata mode is
    the mode that makes the marshalling visible -- the parse itself is ~0.030 ms on
    an M4, the same order as a 785 KiB copy -- so the gap between this pair is the
    cost of accepting a `str`, read off in one round on one machine.

    Guarded like its sibling, for the same reason: the gate measures this
    revision's benchmarks against the BASE revision's build (#168), so a base
    predating `mode=` must skip here rather than raise.
    """
    import pytest

    from fast_mail_parser import parse_email

    try:
        parse_email(large_message, mode="metadata")
    except TypeError:
        pytest.skip("this build predates parse_email(mode=...)")

    benchmark(lambda: parse_email(large_message, mode="metadata"))


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
    # a re-parse of each part's headers before its decode. Since #239 the copy is
    # gone from that list, so the remaining gap is the re-parse alone. Whoever is
    # going to decode everything anyway should use the default mode, and this is
    # the number that says so.
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
    # The other deferred tree mode with nothing read. It retains each leaf's
    # offsets where metadata mode retains nothing at all, so since #239 the gap
    # between this and the benchmark above is the price of *being able* to decode
    # one part later rather than the price of copying it: the two modes now
    # allocate the same amount on the fixtures `bench/tests/allocs.rs` counts.
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

    batch = _distinct(_small_message(), SMALL_BATCH)

    try:
        parse_many(batch[:1], mode="metadata")
    except (TypeError, ValueError):
        pytest.skip('this build predates parse_many(mode="metadata")')

    benchmark(lambda: parse_many(batch, mode="metadata"))
