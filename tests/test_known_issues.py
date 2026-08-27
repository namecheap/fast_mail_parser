"""Characterization tests for known-wrong behaviour.

Unlike the rest of the suite, these assert what the parser currently does, not
what it should do. Each one pins an open bug so that:

- the behaviour is described in executable form rather than only in an issue, and
- fixing it is a deliberate act -- the test fails and has to be updated, instead
  of the fix landing silently and nobody noticing the contract moved.

Every test here names its issue. When that issue is closed, the test should be
rewritten to assert the corrected behaviour, or deleted.

#150 is the first to have been fixed, so its section below is the exception the
rule above describes: those tests assert the recovered behaviour and stay here as
the regression suite for the fix.
"""

import email
import email.policy
import os

import pytest

from fast_mail_parser import parse_email, parse_email_tree, walk

FIXTURE = os.path.join(os.path.dirname(__file__), "data", "invalid_message.eml")

# The boundary declared in the message's own Content-Type header.
BOUNDARY = "_----------=_MCPart_1735325173"


@pytest.fixture(scope="module")
def malformed() -> bytes:
    with open(FIXTURE, "rb") as handle:
        return handle.read()


# --- #150: an unterminated header block is repaired ---------------------------
#
# `invalid_message.eml` is a real Mailchimp-delivered message, bare-LF
# throughout, whose header block is never terminated by a blank line. A folded
# continuation line (` hello`, which makes MIME-Version read `1.0 hello`) is
# followed directly by the FIRST MIME boundary. The stdlib has a name for this
# defect: MissingHeaderBodySeparatorDefect.
#
# Left as it arrived, that message cost us the first part. mailparse stops
# parsing headers only at a blank line and accepts a colonless line as a field
# name, so it consumed 73 lines as headers -- 31 keys, one of them the boundary,
# and two Content-Type values, the message's own and the part's. The boundary
# that opened the text/plain part having been eaten, that part's content ended up
# before the NEXT boundary, which makes it multipart preamble: discarded by
# definition. The text/html part was delimited normally and survived, so the
# message came back looking populated with its plain-text alternative silently
# gone.
#
# `repair_missing_separator` in src/mail_parser.rs now restores the separator the
# sender omitted, by the stdlib's rule: a non-continuation line in the header
# block that cannot be a header field ends the header block, and the body starts
# there. The tests below assert the recovery, the last three against the stdlib
# rather than against numbers written down here.


def test__150_malformed_message_still_parses(malformed: bytes):
    mail = parse_email(malformed)

    assert mail.subject == "Your June OpenShift Update"


def test__150_the_lost_part_is_recovered(malformed: bytes):
    mail = parse_email(malformed)

    # Both alternatives, which is what the message actually contains.
    assert len(mail.text_plain) == 1
    assert len(mail.text_html) == 1
    assert "OpenShift" in mail.text_plain[0]


def test__150_boundary_is_not_a_header_key(malformed: bytes):
    mail = parse_email(malformed)

    assert f"--{BOUNDARY}" not in mail.headers

    # The rule rather than the fixture: no key may be `--` + the boundary this
    # message declares. That signature can only mean unterminated headers.
    content_type = mail.headers["Content-Type"][0]
    declared = content_type.split('boundary="')[1].split('"')[0]
    assert f"--{declared}" not in mail.headers


def test__150_part_headers_stay_out_of_the_top_level_map(malformed: bytes):
    mail = parse_email(malformed)

    # The two lines after the swallowed boundary are the first part's own
    # headers. A message declares Content-Type once.
    assert len(mail.headers["Content-Type"]) == 1
    assert "multipart/alternative" in mail.headers["Content-Type"][0]


def test__150_the_second_defect_is_left_as_it_is(malformed: bytes):
    # The message carries two defects and only one of them loses data. The
    # injected ` hello` line is a legal fold, so it still folds into the header
    # above it -- as it does for the stdlib, which reads the same value.
    mail = parse_email(malformed)

    assert mail.headers["MIME-Version"] == ["1.0 hello"]


def test__150_the_header_set_matches_the_stdlib(malformed: bytes):
    message = email.message_from_bytes(malformed, policy=email.policy.default)

    mail = parse_email(malformed)

    assert set(mail.headers) == set(dict(message.items()))


def test__150_the_recovered_body_matches_the_stdlib(malformed: bytes):
    message = email.message_from_bytes(malformed, policy=email.policy.default)
    theirs = [
        part.get_content()
        for part in message.walk()
        if part.get_content_type() == "text/plain"
        and part.get_content_disposition() != "attachment"
    ]

    ours = parse_email(malformed).text_plain

    # Line endings aside, which differ here for the reason they differ on every
    # fixture: the stdlib normalises body line endings to LF and this library
    # returns the wire form. See docs/compatibility.md.
    assert [text.replace("\r\n", "\n") for text in ours] == theirs


def test__150_the_tree_agrees_with_the_flat_view(malformed: bytes):
    # Both views parse the same payload through the same repair, so the structure
    # one reports cannot contradict the bodies the other returns.
    theirs = email.message_from_bytes(malformed, policy=email.policy.default)

    ours = [part.content_type for part in walk(parse_email_tree(malformed))]

    assert ours == [part.get_content_type() for part in theirs.walk()]


# The repair fires only while scanning the header block, which ends at the first
# blank line -- so a body line without a colon, which is most body lines, can
# never reach it. These pin that boundary.


def test__150_a_terminated_header_block_is_untouched():
    mail = parse_email(
        b"Subject: intact\r\n"
        b"Content-Type: text/plain\r\n"
        b"\r\n"
        b"a body line with no colon in it\r\n"
    )

    assert mail.subject == "intact"
    assert mail.text_plain == ["a body line with no colon in it\r\n"]


def test__150_the_rule_is_the_colon_and_not_the_boundary():
    # Nothing MIME-specific about it: the header block ends at the first line
    # that cannot be a header field, whatever that line happens to say.
    mail = parse_email(b"Subject: no separator\nthis line is the body\n")

    assert mail.subject == "no separator"
    assert mail.text_plain == ["this line is the body\n"]


def test__150_a_folded_continuation_does_not_end_the_header_block():
    # A continuation line carries no colon of its own. Ending the header block at
    # one would be a new bug in the same place as the old one.
    mail = parse_email(
        b"Subject: folded\r\n"
        b"X-Long: first\r\n"
        b" second\r\n"
        b"\r\n"
        b"body\r\n"
    )

    assert mail.headers["X-Long"] == ["first second"]
    assert mail.text_plain == ["body\r\n"]
