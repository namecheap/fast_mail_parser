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
