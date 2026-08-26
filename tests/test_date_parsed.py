"""Tests for `PyMail.date_parsed`.

`date` stays the raw header string; `date_parsed` resolves it to a
timezone-aware `datetime` in UTC, computed on access.

Expected epochs below are taken from the stdlib's own interpretation
(`email.utils.parsedate_to_datetime`). mailparse and the stdlib agree on offset
handling -- mailparse's test suite asserts `17 Sep 2016 16:05:38 -1000 ==
1474164338`, which is exactly what the stdlib computes -- so comparing against
stdlib-derived values is a real cross-check rather than a restatement.
"""

from datetime import datetime, timezone

import pytest

from fast_mail_parser import PyMail, parse_email


def _parse(date_header: str) -> PyMail:
    raw = f"Subject: dates\r\nDate: {date_header}\r\n\r\nbody\r\n"
    return parse_email(raw.encode("ascii"))


# header value -> expected Unix timestamp
VALID = {
    "Mon, 01 Jan 2024 12:00:00 +0000": 1704110400,
    # Same wall-clock, east of UTC: an earlier instant.
    "Mon, 01 Jan 2024 12:00:00 +0200": 1704103200,
    # Different wall-clock and offset, but the SAME instant as the +0000 case.
    "Mon, 01 Jan 2024 07:00:00 -0500": 1704110400,
    # Named zone rather than a numeric offset.
    "Fri, 30 Nov 2012 20:57:23 GMT": 1354309043,
}


@pytest.mark.parametrize("header", list(VALID), ids=list(VALID))
def test__valid_dates_resolve_to_the_right_instant(header: str):
    parsed = _parse(header).date_parsed

    assert parsed is not None
    assert parsed.timestamp() == VALID[header]


@pytest.mark.parametrize("header", list(VALID), ids=list(VALID))
def test__parsed_dates_are_timezone_aware(header: str):
    parsed = _parse(header).date_parsed

    assert parsed is not None
    # tz-aware means utcoffset() is not None; naive datetimes silently misbehave
    # in comparisons and arithmetic, so this is the property that matters.
    assert parsed.utcoffset() is not None
    assert parsed.tzinfo == timezone.utc


def test__equal_instants_in_different_zones_compare_equal():
    utc = _parse("Mon, 01 Jan 2024 12:00:00 +0000").date_parsed
    est = _parse("Mon, 01 Jan 2024 07:00:00 -0500").date_parsed

    assert utc == est


def test__matches_the_stdlib_interpretation():
    # Differential check against email.utils rather than a hardcoded constant.
    from email.utils import parsedate_to_datetime

    for header in VALID:
        assert _parse(header).date_parsed == parsedate_to_datetime(header)


@pytest.mark.parametrize(
    "header",
    [
        "not a date",
        # Recognisable shape, unrecognisable month.
        "Mon, 99 Xxx 2024 99:99:99 +0000",
        "",
        "12345",
        "Subject-like text with no date in it",
    ],
)
def test__unparseable_date_yields_none_with_raw_string_intact(header: str):
    mail = _parse(header)

    assert mail.date_parsed is None
    # The raw header is not lost, and the rest of the message parsed fine.
    assert mail.date == header
    assert mail.subject == "dates"


def test__absent_date_header_yields_none():
    mail = parse_email(b"Subject: no date\r\n\r\nbody\r\n")

    assert mail.date == ""
    assert mail.date_parsed is None


def test__date_parsed_is_a_datetime():
    parsed = _parse("Mon, 01 Jan 2024 12:00:00 +0000").date_parsed

    assert isinstance(parsed, datetime)
    assert parsed.year == 2024
    assert parsed.month == 1
    assert parsed.day == 1
    assert parsed.hour == 12  # reported in UTC


def test__legitimate_epoch_zero_is_not_mistaken_for_a_failure():
    # The guard against `dateparse` reporting Ok(0) for input it never parsed
    # must not reject a date that genuinely IS the epoch.
    mail = _parse("Thu, 01 Jan 1970 00:00:00 +0000")

    assert mail.date_parsed is not None
    assert mail.date_parsed.timestamp() == 0
    assert mail.date_parsed == datetime(1970, 1, 1, tzinfo=timezone.utc)
