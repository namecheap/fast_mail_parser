"""The panic backstop at the FFI boundary (#102).

A Rust panic does not abort the process -- PyO3 catches it and raises
`pyo3_runtime.PanicException`. But that derives from `BaseException`, so the
`except Exception` around a mail pipeline's parse call does not catch it, and one
crafted message takes the worker down with it.

So the backstop is not about memory safety, it is about which `except` clause a
panic lands in. These tests exist because an untested backstop is
indistinguishable from a missing one, and the parser is not known to panic on any
input -- hence the deliberate trigger.
"""
import pytest
from fast_mail_parser.fast_mail_parser import _panic_for_tests

import fast_mail_parser
from fast_mail_parser import ParseError, parse_email


def test__a_panic_surfaces_as_parse_error():
    with pytest.raises(ParseError) as excinfo:
        _panic_for_tests()

    # The payload survives, so the bug stays diagnosable.
    assert "deliberate panic from _panic_for_tests" in str(excinfo.value)
    assert "bug in fast_mail_parser" in str(excinfo.value)


def test__a_panic_is_caught_by_except_exception():
    # The whole point. `PanicException` derives from `BaseException` and would
    # sail straight through this, killing the worker.
    try:
        _panic_for_tests()
    except Exception as error:
        assert isinstance(error, ParseError)
    else:
        pytest.fail("the panic did not raise at all")


def test__the_raised_error_is_not_a_bare_base_exception():
    with pytest.raises(ParseError) as excinfo:
        _panic_for_tests()

    assert isinstance(excinfo.value, Exception)
    assert type(excinfo.value).__name__ != "PanicException"


def test__the_module_still_works_after_a_panic(valid_message: str):
    # Unwinding out of the parser must leave nothing broken behind: a caught
    # panic that poisons the extension would only move the outage later.
    with pytest.raises(ParseError):
        _panic_for_tests()

    mail = parse_email(valid_message)

    assert mail.subject


def test__the_trigger_is_not_part_of_the_public_surface():
    # It ships in the extension because there is no other way to obtain a panic
    # on demand, but it must not look like API.
    assert "_panic_for_tests" not in fast_mail_parser.__all__
    assert not hasattr(fast_mail_parser, "_panic_for_tests")
