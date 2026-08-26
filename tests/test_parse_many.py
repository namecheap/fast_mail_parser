"""Tests for `parse_many`, the batch API.

Contract:
- One result per input, in input order.
- Each slot is a `PyMail` or a `ParseError` *instance* -- returned, not raised --
  so one malformed message does not cost the caller the rest of the batch.
- `raise_on_error=True` raises the first failure instead.
- `threads` caps the worker count.
- The GIL is released for the whole batch, not per message.
"""

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


def test__matches_parse_email_one_by_one():
    payloads = [_message(f"m{i}") for i in range(10)]

    batch = parse_many(payloads)
    single = [parse_email(p) for p in payloads]

    assert [m.subject for m in batch] == [m.subject for m in single]
    assert [m.text_plain for m in batch] == [m.text_plain for m in single]


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
