import pytest
import sys
from typing import Callable

from fast_mail_parser import PyMail, parse_email

sys.path.pop(0)


@pytest.fixture(scope='module')
def read_mail() -> Callable:
    def wrap(path: str):
        with open(path, 'r') as f:
            return f.read()

    return wrap


@pytest.fixture
def attachment_mail(read_mail: Callable) -> PyMail:
    message = read_mail('tests/data/attachment_message.eml')

    return parse_email(message)


@pytest.fixture
def valid_mail(valid_message: str, read_mail: Callable) -> PyMail:
    return parse_email(valid_message)


@pytest.fixture
def large_mail(large_message: str, read_mail: Callable) -> PyMail:
    return parse_email(large_message)


@pytest.fixture
def valid_message(read_mail: Callable) -> str:
    return read_mail('tests/data/valid_message.eml')


@pytest.fixture
def invalid_message(read_mail: Callable) -> str:
    return read_mail('tests/data/invalid_message.eml')


@pytest.fixture(scope='module')
def large_message(read_mail: Callable) -> str:
    return read_mail('tests/data/large_message.eml')
