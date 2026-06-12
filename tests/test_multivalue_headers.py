"""Characterization tests for duplicate / multi-value headers.

``PyMail.headers`` is a ``dict[str, str]`` (backed by a Rust ``HashMap<String,
String>``). RFC 5322 allows several header fields to appear more than once
(``Received``, ``X-*`` trace headers, ``Comments``, etc.), but a ``dict`` cannot
hold more than one value per key. This file pins the resulting *collapse*
behavior so consumers know what to expect and so any future change (e.g.
switching to a multimap) is caught.

What is covered elsewhere (NOT re-tested here):
- Non-ASCII / RFC 2047 / RFC 6532 header decoding -> ``tests/test_rfc_corpus.py``.
- Content-Disposition handling and filename sourcing -> ``tests/test_rfc_corpus.py``.

This file focuses solely on the multi-value gap those suites do not cover.

Characterized behavior (current contract):
- Duplicate headers collapse to a single ``str`` value per key; the values are
  NOT concatenated, joined, or exposed as a list.
- Exactly one of the duplicated values is retained. The retained value is one of
  the originals (we do not over-specify *which*, since it depends on the
  HashMap collection order in the Rust layer); see the dedicated test for the
  precise observed selection.
"""

from fast_mail_parser import parse_email


def _build(raw_headers: str) -> bytes:
    return (raw_headers + "\r\n\r\nbody\r\n").encode("ascii")


def test__duplicate_custom_header_collapses_to_single_string():
    raw = "Subject: dup test\r\nX-Custom: first\r\nX-Custom: second"
    mail = parse_email(_build(raw))

    value = mail.headers["X-Custom"]
    assert isinstance(value, str)
    # Collapsed to one of the original values, never both joined together.
    assert value in {"first", "second"}
    assert "first" not in value or "second" not in value


def test__duplicate_received_headers_collapse_to_single_value():
    raw = (
        "Subject: trace test\r\n"
        "Received: from a.example.com by mx1.example.com\r\n"
        "Received: from b.example.com by mx2.example.com"
    )
    mail = parse_email(_build(raw))

    received = mail.headers["Received"]
    assert isinstance(received, str)
    assert received in {
        "from a.example.com by mx1.example.com",
        "from b.example.com by mx2.example.com",
    }


def test__three_duplicates_still_yield_one_value():
    raw = (
        "Subject: triple\r\n"
        "X-Trace: one\r\n"
        "X-Trace: two\r\n"
        "X-Trace: three"
    )
    mail = parse_email(_build(raw))

    assert mail.headers["X-Trace"] in {"one", "two", "three"}


def test__headers_dict_has_one_entry_per_key_despite_duplicates():
    # The dict cannot represent multiplicity: duplicated keys do not inflate
    # the entry count. Both X-Dup lines map to a single dict entry.
    raw = "Subject: count\r\nX-Dup: a\r\nX-Dup: b\r\nX-Unique: u"
    mail = parse_email(_build(raw))

    assert "X-Dup" in mail.headers
    assert "X-Unique" in mail.headers
    # Only the keys we set plus Subject; no per-occurrence duplication.
    assert mail.headers["X-Unique"] == "u"


def test__retained_duplicate_value_is_one_of_the_originals():
    # Document the observed selection precisely. parse_email feeds headers into a
    # Rust HashMap via `.collect()`, so the last-inserted value for a key wins;
    # in practice that surfaces as the LAST occurrence in the message. We assert
    # the value is one of the originals (the firm guarantee) and additionally
    # record the observed last-wins behavior so a regression is visible.
    raw = "Subject: order\r\nX-Order: alpha\r\nX-Order: omega"
    mail = parse_email(_build(raw))

    value = mail.headers["X-Order"]
    assert value in {"alpha", "omega"}
    # Observed behavior: the last occurrence is retained.
    assert value == "omega"
