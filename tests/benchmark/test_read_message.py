from typing import Callable

import pytest


@pytest.fixture(scope='module')
def large_message(read_mail: Callable) -> str:
    return read_mail('tests/data/large_message.eml')


def test__mail_parser___parse_message(large_message: str, benchmark: Callable):
    from mailparser import MailParser

    benchmark(MailParser.from_string, large_message)


def test__fast_mail_parser___parse_message(large_message: str, benchmark: Callable):
    from fast_mail_parser import parse_email

    benchmark(parse_email, large_message)
