"""Characterization tests for known-wrong behaviour.

Unlike the rest of the suite, these assert what the parser currently does, not
what it should do. Each one pins an open bug so that:

- the behaviour is described in executable form rather than only in an issue, and
- fixing it is a deliberate act -- the test fails and has to be updated, instead
  of the fix landing silently and nobody noticing the contract moved.

Every test here names its issue. When that issue is closed, the test should be
rewritten to assert the corrected behaviour, or deleted.
"""

import email
import email.policy
import os

import pytest

from fast_mail_parser import parse_email

FIXTURE = os.path.join(os.path.dirname(__file__), "data", "invalid_message.eml")

# The boundary declared in the message's own Content-Type header.
BOUNDARY = "_----------=_MCPart_1735325173"


@pytest.fixture(scope="module")
def malformed() -> bytes:
    with open(FIXTURE, "rb") as handle:
        return handle.read()


# --- #150: unterminated header block swallows the body -----------------------
#
# The fixture's header block is never terminated: a folded continuation line
# (` hello`) is followed directly by the FIRST MIME boundary with no blank line.
# That boundary is consumed as a header field, so the part it opened -- the
# text/plain body -- is lost. The second boundary is intact, so the text/html
# part survives. No error is raised, so the loss is silent and partial.


def test__150_malformed_message_still_parses(malformed: bytes):
    # It does not raise. That is the whole problem: the failure is silent.
    mail = parse_email(malformed)

    assert mail.subject, "expected the headers before the break to survive"


def test__150_only_the_first_part_is_lost(malformed: bytes):
    mail = parse_email(malformed)

    # WRONG, pinned: the message has a text/plain part and we report none,
    # because the boundary that opened it was eaten as a header.
    assert mail.text_plain == []
    # The second boundary survived, so the html part is recovered. The loss is
    # partial, which is what makes it easy to miss.
    assert len(mail.text_html) == 1


def test__150_boundary_is_reported_as_a_header_key(malformed: bytes):
    mail = parse_email(malformed)

    # WRONG, pinned: a boundary delimiter is not a header field.
    assert f"--{BOUNDARY}" in mail.headers


def test__150_the_stdlib_recovers_what_we_lose(malformed: bytes):
    # The contrast is the argument that this is our bug and not merely bad input:
    # the same bytes give the stdlib the part we drop.
    message = email.message_from_bytes(malformed, policy=email.policy.default)

    bodies = [
        part
        for part in message.walk()
        if part.get_content_type() == "text/plain"
        and part.get_content_disposition() != "attachment"
    ]

    assert len(bodies) == 1
    assert f"--{BOUNDARY}" not in dict(message.items())


def test__150_a_header_key_matches_the_declared_boundary(malformed: bytes):
    # The signal a fix could detect cheaply and exactly: a header key equal to
    # `--<the boundary this message declares>` can only mean the header block was
    # never terminated. Well-formed mail cannot produce it.
    mail = parse_email(malformed)

    content_type = mail.headers["Content-Type"][0]
    assert content_type.startswith("multipart/")

    declared = content_type.split('boundary="')[1].split('"')[0]
    assert f"--{declared}" in mail.headers, (
        "the unterminated-header signature: the declared boundary appears as a "
        "header key"
    )
