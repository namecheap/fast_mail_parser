import sys
from collections.abc import Callable

import pytest

from fast_mail_parser import PyMail, parse_email

sys.path.pop(0)


@pytest.fixture(scope='module')
def read_mail() -> Callable:
    def wrap(path: str):
        with open(path) as f:
            return f.read()

    return wrap


@pytest.fixture(scope='module')
def attachment_mail(read_mail: Callable) -> PyMail:
    message = read_mail('tests/data/attachment_message.eml')

    return parse_email(message)


@pytest.fixture(scope='module')
def valid_mail(valid_message: str, read_mail: Callable) -> PyMail:
    return parse_email(valid_message)


@pytest.fixture(scope='module')
def large_mail(large_message: str, read_mail: Callable) -> PyMail:
    return parse_email(large_message)


@pytest.fixture(scope='module')
def valid_message(read_mail: Callable) -> str:
    return read_mail('tests/data/valid_message.eml')


@pytest.fixture
def invalid_message(read_mail: Callable) -> str:
    """A malformed real-world message that nonetheless PARSES.

    The name is misleading and has cost real time: `parse_email` does not raise
    on this file. It is malformed in that its header block is never terminated --
    a folded continuation line (` hello`) is followed directly by the MIME
    boundary with no blank line between them.

    The consequence is silent: the boundary is consumed as a header field, so the
    `multipart/alternative` declared in Content-Type finds zero parts and the body
    disappears. `text_plain` comes back empty and `headers` contains one entry
    whose key is the boundary delimiter. The stdlib recovers the body; we do not.
    Tracked in #150.

    So this is not a substitute for a parse failure. Use an inline payload with a
    broken transfer encoding for that -- see `tests/test_error_taxonomy.py`.
    The stdlib-parity suite excludes this fixture for the same reason: the two
    parsers disagree about it rather than either rejecting it.
    """
    return read_mail('tests/data/invalid_message.eml')


@pytest.fixture(scope='module')
def large_message(read_mail: Callable) -> str:
    return read_mail('tests/data/large_message.eml')
