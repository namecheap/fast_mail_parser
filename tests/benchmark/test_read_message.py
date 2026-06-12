from typing import Callable


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
