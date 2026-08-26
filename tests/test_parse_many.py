"""Tests for `parse_many`, the batch API.

Contract:
- One result per input, in input order.
- Each slot is a `PyMail` or a `ParseError` *instance* -- returned, not raised --
  so one malformed message does not cost the caller the rest of the batch.
- `raise_on_error=True` raises the first failure instead.
- `threads` caps the worker count.
- The GIL is released for the whole batch, not per message.
"""

import glob
import os
import threading
import time

import pytest

from fast_mail_parser import DecodeError, ParseError, PyMail, parse_email, parse_many


def _message(subject: str) -> bytes:
    return (
        f"Subject: {subject}\r\n"
        "From: sender@example.com\r\n"
        "Content-Type: text/plain; charset=utf-8\r\n"
        "\r\n"
        f"body of {subject}\r\n"
    ).encode()


BROKEN = (
    b"Subject: broken\r\n"
    b"Content-Type: text/plain; charset=utf-8\r\n"
    b"Content-Transfer-Encoding: base64\r\n"
    b"\r\n"
    b"!!!! not base64 !!!!\r\n"
)


# --- ordering and basic shape -------------------------------------------------


def test__empty_batch_returns_empty_list():
    assert parse_many([]) == []


def test__results_are_in_input_order():
    payloads = [_message(f"msg-{i:03d}") for i in range(50)]

    results = parse_many(payloads)

    assert len(results) == len(payloads)
    assert [r.subject for r in results] == [f"msg-{i:03d}" for i in range(50)]


def test__accepts_mixed_str_and_bytes():
    payloads = [_message("as-bytes"), _message("as-str").decode("utf-8")]

    results = parse_many(payloads)

    assert [r.subject for r in results] == ["as-bytes", "as-str"]
    assert all(isinstance(r, PyMail) for r in results)


_DATA = os.path.join(os.path.dirname(__file__), "data")

# Every real fixture. invalid_message.eml is excluded because the two parsers
# disagree about it -- it is malformed in a way both accept but interpret
# differently -- not because it fails to parse; it does parse.
CORPUS = sorted(glob.glob(os.path.join(_DATA, "rfc", "*.eml"))) + sorted(
    path
    for path in glob.glob(os.path.join(_DATA, "*.eml"))
    if os.path.basename(path) != "invalid_message.eml"
)


def _view(mail: PyMail) -> dict:
    """Everything a caller can observe, for comparing two parses of one message."""
    return {
        "subject": mail.subject,
        "date": mail.date,
        "date_parsed": mail.date_parsed,
        "text_plain": tuple(mail.text_plain),
        "text_html": tuple(mail.text_html),
        "attachments": [
            (a.mimetype, a.filename, a.content, a.content_id, a.disposition)
            for a in mail.attachments
        ],
        "headers": {key: list(values) for key, values in mail.headers.items()},
        "from_": None if mail.from_ is None else (mail.from_.display_name, mail.from_.address),
        "to": [(a.display_name, a.address) for a in mail.to],
        "cc": [(a.display_name, a.address) for a in mail.cc],
    }


def test__batch_matches_single_parse_across_the_whole_corpus():
    # The batch path must be indistinguishable from calling parse_email in a
    # loop -- on every observable field, over real messages, not just subjects.
    assert len(CORPUS) >= 15, f"expected the fixture corpus, found {len(CORPUS)}"
    payloads = [open(path, "rb").read() for path in CORPUS]

    batch = parse_many(payloads)

    assert len(batch) == len(payloads)
    # strict: equal lengths are part of the contract being asserted.
    for path, payload, batched in zip(CORPUS, payloads, batch, strict=True):
        assert _view(batched) == _view(parse_email(payload)), (
            f"batch and single parse disagree on {os.path.basename(path)}"
        )


def test__batch_results_do_not_depend_on_position():
    # Same messages, reversed order: each message's own result must be unchanged.
    # Catches a worker writing into the wrong slot in a way a uniform batch hides.
    payloads = [open(path, "rb").read() for path in CORPUS]

    forward = parse_many(payloads)
    backward = parse_many(list(reversed(payloads)))

    assert [_view(m) for m in forward] == [_view(m) for m in reversed(backward)]


def test__chunking_a_batch_changes_nothing():
    # No state may leak between messages within a call, or between calls.
    payloads = [open(path, "rb").read() for path in CORPUS]

    whole = [_view(m) for m in parse_many(payloads)]
    chunked = []
    for start in range(0, len(payloads), 4):
        chunked.extend(_view(m) for m in parse_many(payloads[start:start + 4]))

    assert whole == chunked


def test__an_unparseable_payload_lands_in_its_own_slot_amid_real_messages():
    # BROKEN rather than tests/data/invalid_message.eml: despite its name that
    # fixture parses successfully (see #150), so it cannot
    # stand in for a parse failure.
    good = open(CORPUS[0], "rb").read()
    payloads = [good, BROKEN, good]

    results = parse_many(payloads)

    assert isinstance(results[1], ParseError)
    assert _view(results[0]) == _view(results[2]) == _view(parse_email(good))


# --- per-item errors ----------------------------------------------------------


def test__one_malformed_message_does_not_fail_the_batch():
    payloads = [_message("first"), BROKEN, _message("third")]

    results = parse_many(payloads)

    assert len(results) == 3
    assert results[0].subject == "first"
    assert isinstance(results[1], ParseError), "the error belongs in its own slot"
    assert results[2].subject == "third"


def test__the_error_slot_holds_the_specific_subtype():
    results = parse_many([BROKEN])

    assert isinstance(results[0], DecodeError)
    assert isinstance(results[0], ParseError)
    assert "Message parsing error" in str(results[0])


def test__errors_land_in_the_right_slots():
    # Broken at every third position, so a mis-ordered implementation shows up.
    payloads = [BROKEN if i % 3 == 0 else _message(f"ok-{i}") for i in range(12)]

    results = parse_many(payloads)

    for index, result in enumerate(results):
        if index % 3 == 0:
            assert isinstance(result, ParseError), f"slot {index} should be an error"
        else:
            assert result.subject == f"ok-{index}", f"slot {index} misplaced"


def test__raise_on_error_raises_the_first_failure():
    payloads = [_message("fine"), BROKEN, _message("also fine")]

    with pytest.raises(DecodeError):
        parse_many(payloads, raise_on_error=True)


def test__raise_on_error_is_quiet_when_everything_parses():
    payloads = [_message("a"), _message("b")]

    results = parse_many(payloads, raise_on_error=True)

    assert [r.subject for r in results] == ["a", "b"]


# --- threads ------------------------------------------------------------------


@pytest.mark.parametrize("threads", [1, 2, 4, 64])
def test__thread_cap_does_not_change_results(threads: int):
    payloads = [_message(f"t-{i}") for i in range(30)]

    results = parse_many(payloads, threads=threads)

    assert [r.subject for r in results] == [f"t-{i}" for i in range(30)]


def test__batch_smaller_than_the_thread_cap_is_fine():
    # Must not spawn idle workers or mis-handle the short batch.
    results = parse_many([_message("only")], threads=32)

    assert [r.subject for r in results] == ["only"]


# --- the GIL is released for the batch ----------------------------------------


def test__gil_is_released_for_the_duration_of_the_batch():
    # A Python thread must keep making progress while a large batch parses. If
    # the GIL were held for the batch, this counter would barely move.
    # Pattern follows the single-message GIL test from #91.
    payloads = [_message(f"gil-{i}") * 20 for i in range(400)]
    counter = 0
    stop = False

    def spin():
        nonlocal counter
        while not stop:
            counter += 1
            time.sleep(0)

    ticker = threading.Thread(target=spin, daemon=True)
    ticker.start()
    try:
        before = counter
        parse_many(payloads)
        progressed = counter - before
    finally:
        stop = True
        ticker.join(timeout=5)

    assert progressed > 0, (
        "a Python thread made no progress during the batch, so the GIL was held"
    )


# --- argument contract -------------------------------------------------------


def test__threads_zero_is_rejected():
    # Silently meaning "the default" would hide a caller bug such as
    # threads=os.cpu_count() - 1 on a one-core machine.
    with pytest.raises(ValueError, match="threads must be at least 1"):
        parse_many([_message("x")], threads=0)


def test__negative_threads_is_rejected():
    # `threads` is unsigned on the Rust side, so this fails at conversion.
    with pytest.raises(OverflowError):
        parse_many([_message("x")], threads=-1)


def test__absurdly_large_thread_cap_is_harmless():
    # Workers never outnumber the batch, so this must not try to spawn a billion.
    results = parse_many([_message("a"), _message("b")], threads=10**9)

    assert [r.subject for r in results] == ["a", "b"]


@pytest.mark.parametrize(
    "bad", [123, None, object()], ids=["int", "None", "object"]
)
def test__non_payload_entries_raise_type_error(bad: object):
    with pytest.raises(TypeError):
        parse_many([_message("ok"), bad])


def test__a_sequence_other_than_a_list_is_accepted():
    # The signature says list, but any Sequence works; a tuple is the common one.
    results = parse_many((_message("a"), _message("b")))

    assert [r.subject for r in results] == ["a", "b"]


def test__threads_and_raise_on_error_are_keyword_only():
    # Positional booleans at a call site read poorly, so they are keyword-only.
    with pytest.raises(TypeError):
        parse_many([_message("x")], 2)
