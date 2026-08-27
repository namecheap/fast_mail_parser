"""Tests for the parse-warnings channel and strict mode (#100).

The parser has always been best-effort: an unrecognised charset label, an
address header that does not parse, an unreadable `Date` all produce a result
rather than an exception. What was missing is any way for the caller to tell a
pristine parse from a patched-up one. `PyMail.warnings` is that channel, and the
contract worth having is the empty list: `warnings == []` means nothing was
repaired, so anything else can be routed to quarantine or review.

Two properties are asserted for every kind, because either alone is misleading:

- the warning is emitted, with the expected `(kind, part_path)`, and
- the best-effort content is still *correct* alongside it -- a warning is not a
  licence to return worse output.

`strict=True` is the same set of conditions raised instead of recorded, so it is
parametrised over the same payloads rather than given its own fixtures.
"""

import glob
import os

import pytest

from fast_mail_parser import (
    DecodeError,
    HeaderParseError,
    ParseError,
    ParseWarning,
    parse_email,
    parse_many,
)

DATA = os.path.join(os.path.dirname(__file__), "data")

# `x-unknown` is the label mailparse's own test suite pins as one the charset
# crate does not resolve, which is what makes it reach the us-ascii fallback.
# Chosen over inventing a label because "unrecognised" is upstream's judgement,
# not ours.
UNKNOWN_CHARSET = b"x-unknown"

# The body is pure ASCII, so the fallback decode is lossless *here* -- which is
# the point of asserting the content: the warning says a repair happened, not
# that the result is wrong.
CHARSET_FALLBACK = (
    b"Subject: fallback\r\n"
    b"Content-Type: text/plain; charset=" + UNKNOWN_CHARSET + b"\r\n"
    b"\r\n"
    b"plain ascii body\r\n"
)

# Same, but the body carries UTF-8 bytes. Decoded as us-ascii, each non-ASCII
# byte becomes U+FFFD -- a real loss, and exactly what the warning is for.
CHARSET_FALLBACK_LOSSY = (
    b"Subject: fallback\r\n"
    b"Content-Type: text/plain; charset=" + UNKNOWN_CHARSET + b"\r\n"
    b"\r\n"
    b"caf\xc3\xa9"
)

CHARSET_FALLBACK_HTML = (
    b"Subject: fallback\r\n"
    b"Content-Type: text/html; charset=" + UNKNOWN_CHARSET + b"\r\n"
    b"\r\n"
    b"<p>hi</p>"
)

# Two text/plain parts, only the second with a bad charset, so `part_path` has
# to name the second slot of `text_plain` rather than just "a part".
CHARSET_FALLBACK_SECOND_PART = (
    b"Subject: two parts\r\n"
    b'Content-Type: multipart/mixed; boundary="b"\r\n'
    b"\r\n"
    b"--b\r\n"
    b"Content-Type: text/plain; charset=utf-8\r\n"
    b"\r\n"
    b"first\r\n"
    b"--b\r\n"
    b"Content-Type: text/plain; charset=" + UNKNOWN_CHARSET + b"\r\n"
    b"\r\n"
    b"second\r\n"
    b"--b--\r\n"
)

# mailparse rejects an address with no `@`, which is the malformed case the rest
# of the suite already uses for this path.
BAD_ADDRESS = b"Subject: addresses\r\nTo: not-an-address\r\n\r\nbody\r\n"

# No month token anywhere, so `dateparse` never advances and the header yields
# nothing rather than a bogus 1970 instant.
BAD_DATE = b"Subject: dates\r\nDate: not a date\r\n\r\nbody\r\n"

# One message that trips three separate repairs, to pin the order they are
# recorded in: header-level repairs as the headers are read, then the body parts.
EVERYTHING_BROKEN = (
    b"Subject: all of it\r\n"
    b"To: not-an-address\r\n"
    b"Date: not a date\r\n"
    b"Content-Type: text/plain; charset=" + UNKNOWN_CHARSET + b"\r\n"
    b"\r\n"
    b"body\r\n"
)

CLEAN = (
    b"Subject: clean\r\n"
    b"From: Sender <sender@example.com>\r\n"
    b"To: Recipient <recipient@example.com>\r\n"
    b"Date: Mon, 01 Jan 2024 12:00:00 +0000\r\n"
    b"Content-Type: text/plain; charset=utf-8\r\n"
    b"\r\n"
    b"body\r\n"
)


def _kinds(payload: bytes) -> list[str]:
    return [w.kind for w in parse_email(payload).warnings]


def _pairs(payload: bytes) -> list[tuple[str, str]]:
    return [(w.kind, w.part_path) for w in parse_email(payload).warnings]


# --- the empty-list contract -------------------------------------------------

# `invalid_message.eml` is excluded deliberately. It is the #150 fixture: a real
# message whose header block is never terminated, so its first body part is
# silently lost. Today that loss produces no warning, which is precisely the gap
# this channel exists to close -- the repair for #150 is what will add the kind
# that fires here, and pinning `warnings == []` for it would then be a test
# fighting its own fix.
CORPUS = sorted(
    path
    for path in glob.glob(os.path.join(DATA, "*.eml"))
    + glob.glob(os.path.join(DATA, "rfc", "*.eml"))
    if os.path.basename(path) != "invalid_message.eml"
)


def test__the_corpus_is_not_empty():
    # A glob that silently matched nothing would make the assertion below
    # vacuous, which is the failure mode of every corpus-wide test.
    assert len(CORPUS) > 10


@pytest.mark.parametrize("path", CORPUS, ids=lambda p: os.path.basename(p))
def test__well_formed_fixtures_warn_about_nothing(path: str):
    with open(path, "rb") as handle:
        mail = parse_email(handle.read())

    assert mail.warnings == [], (
        "no warning inflation on good mail: "
        f"{[(w.kind, w.part_path, w.detail) for w in mail.warnings]}"
    )


@pytest.mark.parametrize("path", CORPUS, ids=lambda p: os.path.basename(p))
def test__well_formed_fixtures_pass_strict_mode(path: str):
    # The corollary: if the corpus warns about nothing, strict mode must accept
    # all of it. A strict mode that rejects ordinary mail is not usable.
    with open(path, "rb") as handle:
        # Reaching this line at all is the assertion: strict mode raises rather
        # than returning when anything was repaired.
        assert parse_email(handle.read(), strict=True).warnings == []


def test__a_clean_message_warns_about_nothing():
    assert parse_email(CLEAN).warnings == []


# --- charset-fallback ---------------------------------------------------------


def test__unrecognised_charset_is_reported():
    mail = parse_email(CHARSET_FALLBACK)

    assert _pairs(CHARSET_FALLBACK) == [("charset-fallback", "text_plain[0]")]
    # The best-effort content is still right: the body was ASCII, so decoding it
    # as us-ascii lost nothing.
    assert mail.text_plain[0].startswith("plain ascii body")
    # The label the parser could not resolve is named, so the warning is
    # actionable without re-reading the message.
    assert "x-unknown" in mail.warnings[0].detail


def test__the_charset_fallback_loss_is_visible_in_the_content():
    mail = parse_email(CHARSET_FALLBACK_LOSSY)

    assert _kinds(CHARSET_FALLBACK_LOSSY) == ["charset-fallback"]
    # decode_ascii replaces each non-ASCII byte with U+FFFD, so the two bytes of
    # the UTF-8 'e-acute' become two replacement characters. This is the repair
    # the warning is reporting, asserted rather than described. Spelled with an
    # escape so the expectation cannot be broken by an editor's encoding.
    assert mail.text_plain[0] == "caf" + "\ufffd" * 2


def test__an_html_part_reports_its_own_slot():
    mail = parse_email(CHARSET_FALLBACK_HTML)

    assert _pairs(CHARSET_FALLBACK_HTML) == [("charset-fallback", "text_html[0]")]
    assert mail.text_html[0] == "<p>hi</p>"


def test__part_path_names_the_slot_the_content_landed_in():
    mail = parse_email(CHARSET_FALLBACK_SECOND_PART)

    assert len(mail.text_plain) == 2
    assert _pairs(CHARSET_FALLBACK_SECOND_PART) == [
        ("charset-fallback", "text_plain[1]"),
    ]
    # The locator resolves: index into the list it names and you get the part
    # the warning is about. That is the property that makes it worth having.
    warning = mail.warnings[0]
    field, index = warning.part_path.rstrip("]").split("[")
    assert getattr(mail, field)[int(index)].startswith("second")


# --- address-unparseable -----------------------------------------------------


def test__unparseable_address_header_is_reported():
    mail = parse_email(BAD_ADDRESS)

    assert _pairs(BAD_ADDRESS) == [("address-unparseable", "")]
    # Unchanged best-effort behaviour: no exception, no mailboxes, raw value kept.
    assert mail.to == []
    assert mail.headers["To"] == ["not-an-address"]
    assert mail.subject == "addresses"
    # Which header it was, since a message has five of them.
    assert "To" in mail.warnings[0].detail


def test__an_absent_address_header_is_not_a_repair():
    # Nothing was dropped, so nothing is reported. The distinction matters: a
    # channel that fires on absent headers would warn about almost every message
    # and be ignored within a week.
    mail = parse_email(b"Subject: no recipients\r\n\r\nbody\r\n")

    assert mail.to == []
    assert mail.warnings == []


def test__each_address_header_is_reported_separately():
    payload = (
        b"Subject: several\r\n"
        b"To: not-an-address\r\n"
        b"Cc: also-not-one\r\n"
        b"\r\nbody\r\n"
    )

    warnings = parse_email(payload).warnings

    assert [w.kind for w in warnings] == ["address-unparseable"] * 2
    assert "To" in warnings[0].detail
    assert "Cc" in warnings[1].detail


# --- date-unparseable --------------------------------------------------------


def test__unparseable_date_is_reported():
    mail = parse_email(BAD_DATE)

    assert _pairs(BAD_DATE) == [("date-unparseable", "")]
    # Best-effort content: the raw header survives, only the interpretation is
    # missing. That is what made this silent before.
    assert mail.date == "not a date"
    assert mail.date_parsed is None


def test__an_absent_date_header_is_not_a_repair():
    mail = parse_email(b"Subject: no date\r\n\r\nbody\r\n")

    assert mail.date == ""
    assert mail.date_parsed is None
    assert mail.warnings == []


def test__a_parseable_date_is_not_a_repair():
    mail = parse_email(BAD_DATE.replace(b"not a date", b"Mon, 01 Jan 2024 12:00:00 +0000"))

    assert mail.date_parsed is not None
    assert mail.warnings == []


# --- several repairs in one message ------------------------------------------


def test__every_repair_is_recorded_in_visit_order():
    # Not a set: the order is the order the parser meets them, headers first and
    # then the body parts, and pinning it keeps `warnings[0]` -- which is what
    # strict mode reports -- deterministic.
    assert _pairs(EVERYTHING_BROKEN) == [
        ("address-unparseable", ""),
        ("date-unparseable", ""),
        ("charset-fallback", "text_plain[0]"),
    ]


def test__repr_names_the_kind():
    warning = parse_email(BAD_DATE).warnings[0]

    assert isinstance(warning, ParseWarning)
    assert "date-unparseable" in repr(warning)


# --- strict mode -------------------------------------------------------------

# Each warning kind maps to the subtype from the #135 hierarchy that describes
# it: a dropped address list is a header failure, a charset fallback or an
# unreadable date is a value that could not be decoded. No new exception types.
STRICT_CASES = {
    "charset-fallback": (CHARSET_FALLBACK, DecodeError),
    "address-unparseable": (BAD_ADDRESS, HeaderParseError),
    "date-unparseable": (BAD_DATE, DecodeError),
}


@pytest.mark.parametrize("kind", list(STRICT_CASES), ids=list(STRICT_CASES))
def test__strict_mode_raises_the_mapped_subtype(kind: str):
    payload, expected = STRICT_CASES[kind]

    # The same payload warns by default...
    assert _kinds(payload) == [kind]
    # ...and raises under strict.
    with pytest.raises(expected):
        parse_email(payload, strict=True)


@pytest.mark.parametrize("kind", list(STRICT_CASES), ids=list(STRICT_CASES))
def test__strict_rejections_are_parse_errors(kind: str):
    # The compatibility promise: a pipeline catching ParseError keeps catching
    # everything, strict mode included.
    payload, _ = STRICT_CASES[kind]

    with pytest.raises(ParseError):
        parse_email(payload, strict=True)


@pytest.mark.parametrize("kind", list(STRICT_CASES), ids=list(STRICT_CASES))
def test__strict_messages_name_the_kind_and_the_mode(kind: str):
    payload, _ = STRICT_CASES[kind]

    try:
        parse_email(payload, strict=True)
    except ParseError as exc:
        assert "strict mode" in str(exc)
        assert kind in str(exc)
    else:
        pytest.fail("strict mode accepted a lossy parse")


def test__strict_is_off_by_default():
    # Default behaviour is unchanged, which is the whole point of it being a
    # keyword argument rather than a new default.
    assert parse_email(BAD_DATE).date == "not a date"


def test__strict_reports_the_first_repair_and_counts_them_all():
    try:
        parse_email(EVERYTHING_BROKEN, strict=True)
    except ParseError as exc:
        assert "address-unparseable" in str(exc)
        assert "3" in str(exc)
    else:
        pytest.fail("strict mode accepted a lossy parse")


def test__strict_does_not_change_which_messages_parse():
    # A message that fails outright fails the same way in both modes, with the
    # same type -- strict mode adds rejections, it does not reclassify.
    broken = b" unexpected continuation\r\n\r\nbody"

    with pytest.raises(HeaderParseError):
        parse_email(broken)
    with pytest.raises(HeaderParseError):
        parse_email(broken, strict=True)


# --- parse_many integration --------------------------------------------------


def test__parse_many_carries_warnings_per_message():
    results = parse_many([CLEAN, BAD_DATE, CHARSET_FALLBACK])

    assert [len(mail.warnings) for mail in results] == [0, 1, 1]
    assert results[1].warnings[0].kind == "date-unparseable"
    assert results[2].warnings[0].part_path == "text_plain[0]"


def test__parse_many_strict_turns_a_lossy_parse_into_that_slot():
    results = parse_many([CLEAN, BAD_DATE], strict=True)

    # Slot 0 parsed; slot 1 holds the exception instance rather than raising, so
    # one repaired message does not cost the caller the rest of the batch.
    assert results[0].warnings == []
    assert isinstance(results[1], DecodeError)
    assert "strict mode" in str(results[1])


def test__parse_many_strict_with_raise_on_error_fails_the_batch():
    with pytest.raises(DecodeError):
        parse_many([CLEAN, BAD_DATE], strict=True, raise_on_error=True)


def test__parse_many_strict_is_off_by_default():
    results = parse_many([CLEAN, BAD_DATE])

    assert not isinstance(results[1], ParseError)
    assert results[1].warnings[0].kind == "date-unparseable"


def test__parse_many_agrees_with_parse_email_on_warnings():
    # The two APIs must not disagree about what a message's repairs are; the
    # batch path is a different traversal of the same core.
    payloads = [CLEAN, BAD_ADDRESS, BAD_DATE, CHARSET_FALLBACK, EVERYTHING_BROKEN]

    batch = parse_many(payloads)

    for payload, mail in zip(payloads, batch, strict=True):
        expected = [(w.kind, w.part_path) for w in parse_email(payload).warnings]
        assert [(w.kind, w.part_path) for w in mail.warnings] == expected
