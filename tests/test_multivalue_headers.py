"""Tests for duplicate / multi-value headers.

``PyMail.headers`` is a ``dict[str, list[str]]`` (backed by a Rust
``HashMap<String, Vec<String>>``). RFC 5322 allows several header fields to
appear more than once (``Received``, ``X-*`` trace headers, ``Comments``, etc.),
and every occurrence is preserved in the order it appeared in the message.

What is covered elsewhere (NOT re-tested here):
- Non-ASCII / RFC 2047 / RFC 6532 header decoding -> ``tests/test_rfc_corpus.py``.
- Content-Disposition handling and filename sourcing -> ``tests/test_rfc_corpus.py``.

Contract:
- Every value is a ``list[str]``, including single-valued headers, which are
  one-element lists.
- Repeated keys keep every value in message order -- nothing is dropped, joined,
  or reordered.
- A repeated key is still a single dict entry; the multiplicity lives in the list.
"""

from fast_mail_parser import PyMail, parse_email


def _build(raw_headers: str) -> bytes:
    return (raw_headers + "\r\n\r\nbody\r\n").encode("ascii")


def test__duplicate_custom_header_keeps_every_value_in_order():
    raw = "Subject: dup test\r\nX-Custom: first\r\nX-Custom: second"
    mail = parse_email(_build(raw))

    assert mail.headers["X-Custom"] == ["first", "second"]


def test__duplicate_received_headers_are_all_returned_in_order():
    # The delivery path: for Received the FIRST occurrence is the most recent
    # hop, so order carries meaning and the earlier hops must not be discarded.
    raw = (
        "Subject: trace test\r\n"
        "Received: from a.example.com by mx1.example.com\r\n"
        "Received: from b.example.com by mx2.example.com\r\n"
        "Received: from c.example.com by mx3.example.com"
    )
    mail = parse_email(_build(raw))

    assert mail.headers["Received"] == [
        "from a.example.com by mx1.example.com",
        "from b.example.com by mx2.example.com",
        "from c.example.com by mx3.example.com",
    ]


def test__three_duplicates_yield_three_values():
    raw = "Subject: triple\r\nX-Trace: one\r\nX-Trace: two\r\nX-Trace: three"
    mail = parse_email(_build(raw))

    assert mail.headers["X-Trace"] == ["one", "two", "three"]


def test__single_valued_header_is_a_one_element_list():
    # Uniform shape: callers never have to branch on str-vs-list.
    raw = "Subject: single\r\nX-Only: solo"
    mail = parse_email(_build(raw))

    assert mail.headers["X-Only"] == ["solo"]
    assert mail.headers["Subject"] == ["single"]


def test__duplicate_keys_do_not_inflate_the_entry_count():
    # Multiplicity lives in the list, not in extra dict entries.
    raw = "Subject: count\r\nX-Dup: a\r\nX-Dup: b\r\nX-Unique: u"
    mail = parse_email(_build(raw))

    assert set(mail.headers) == {"Subject", "X-Dup", "X-Unique"}
    assert mail.headers["X-Dup"] == ["a", "b"]
    assert mail.headers["X-Unique"] == ["u"]


def test__every_value_is_a_list_of_str():
    raw = "Subject: types\r\nX-A: 1\r\nX-A: 2\r\nX-B: 3"
    mail = parse_email(_build(raw))

    for key, values in mail.headers.items():
        assert isinstance(key, str)
        assert isinstance(values, list)
        assert values, f"{key}: a present header must have at least one value"
        assert all(isinstance(v, str) for v in values)


def test__repeated_header_in_real_fixture_is_preserved(attachment_mail: PyMail):
    # tests/data/attachment_message.eml carries two Received-SPF lines.
    assert attachment_mail.headers["Received-SPF"] == ["custom_header1", "custom_header2"]


def test__subject_and_date_are_independent_of_the_headers_map():
    # subject/date are read from the parsed headers directly, so a duplicated
    # Subject cannot make them disagree with the first occurrence.
    raw = "Subject: the real one\r\nSubject: a later duplicate\r\nDate: Mon, 01 Jan 2024 12:00:00 +0000"
    mail = parse_email(_build(raw))

    assert mail.subject == "the real one"
    assert mail.date == "Mon, 01 Jan 2024 12:00:00 +0000"
    # The map still reports both occurrences.
    assert mail.headers["Subject"] == ["the real one", "a later duplicate"]
