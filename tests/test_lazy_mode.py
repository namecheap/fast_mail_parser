"""`parse_email(payload, mode="lazy")` (#97).

Lazy mode decodes the bodies as full mode does and defers each attachment:
`PyLazyAttachment.content` decodes on first access and caches. So the contract
has three halves, and this file is organised as them.

**What it reports must equal full mode.** Not "be equivalent to" -- the decoded
bytes of every attachment in the whole corpus must be equal, and the envelope and
the warning list must be equal too, because a pipeline that switches modes to save
work must not switch what it reads. That equality is also what makes the
implementation trustworthy: lazy mode keeps a copy of each part exactly as it sits
in the message and re-parses that copy on access, which reproduces the full
parse's `content` only if `raw_bytes` really is the part and nothing else. The
`parse_agreement` fuzz target asserts the same thing on arbitrary input.

**The cache must be one object.** `a.content is a.content`, and the same across
threads: a cache that hands back an equal copy is not a cache, it is a decode with
extra steps.

**An attachment nobody reads must never be decoded.** `is_decoded` makes that an
assertion rather than a timing argument.
"""
import glob
import os
import threading

import pytest

from fast_mail_parser import (
    DecodeError,
    PyLazyAttachment,
    PyLazyMail,
    PyMail,
    parse_email,
)

_DATA = os.path.join(os.path.dirname(__file__), "data")

# Every fixture, on the same terms as the metadata-mode corpus test: the modes
# have to agree about `invalid_message.eml` -- whose header block is never
# terminated -- like they agree about any other message.
FIXTURES = sorted(glob.glob(os.path.join(_DATA, "rfc", "*.eml"))) + sorted(
    glob.glob(os.path.join(_DATA, "*.eml"))
)
IDS = [os.path.basename(path) for path in FIXTURES]

EXPECTED_LAZY_ATTACHMENT_ATTRS = {
    "mimetype",
    "filename",
    "content_id",
    "disposition",
    "encoded_size",
    "content",
    "is_decoded",
}

# A part whose transfer encoding cannot be decoded. Full mode raises for it at
# parse time; lazy mode has nothing to decode until someone asks.
BROKEN_ATTACHMENT = (
    b"Subject: broken\r\n"
    b"Content-Type: application/octet-stream; name=\"x.bin\"\r\n"
    b"Content-Disposition: attachment\r\n"
    b"Content-Transfer-Encoding: base64\r\n"
    b"\r\n"
    b"!!!! not base64 !!!!\r\n"
)

CLEAN = b"Subject: clean\r\nDate: Thu, 01 Jan 1970 00:00:00 +0000\r\n\r\nbody\r\n"
BAD_DATE = b"Subject: lossy\r\nDate: not a date\r\n\r\nbody\r\n"


def _read(path: str) -> bytes:
    with open(path, "rb") as handle:
        return handle.read()


def _both(path: str) -> tuple[PyMail, PyLazyMail]:
    raw = _read(path)
    return parse_email(raw), parse_email(raw, mode="lazy")


# --- equality with full mode --------------------------------------------------


@pytest.mark.parametrize("path", FIXTURES, ids=IDS)
def test__attachment_content_is_identical_to_full_mode(path: str):
    # The criterion this feature lives or dies by.
    full, lazy = _both(path)

    assert len(lazy.attachments) == len(full.attachments)
    for deferred, decoded in zip(lazy.attachments, full.attachments, strict=True):
        assert isinstance(deferred, PyLazyAttachment)
        assert deferred.content == decoded.content, deferred.filename


@pytest.mark.parametrize("path", FIXTURES, ids=IDS)
def test__the_attachment_inventory_is_identical_to_full_mode(path: str):
    full, lazy = _both(path)

    for deferred, decoded in zip(lazy.attachments, full.attachments, strict=True):
        assert deferred.mimetype == decoded.mimetype
        assert deferred.filename == decoded.filename
        assert deferred.content_id == decoded.content_id
        assert deferred.disposition == decoded.disposition


@pytest.mark.parametrize("path", FIXTURES, ids=IDS)
def test__the_envelope_and_bodies_are_identical_to_full_mode(path: str):
    # Lazy mode defers attachment content and nothing else, so everything the
    # full parse reports besides that has to be the same value.
    full, lazy = _both(path)

    assert lazy.subject == full.subject
    assert lazy.date == full.date
    assert lazy.date_parsed == full.date_parsed
    assert lazy.headers == full.headers
    assert lazy.text_plain == full.text_plain
    assert lazy.text_html == full.text_html

    def flatten(addresses):
        return [(a.display_name, a.address) for a in addresses]

    assert (lazy.from_ is None) == (full.from_ is None)
    if full.from_ is not None:
        assert (lazy.from_.display_name, lazy.from_.address) == (
            full.from_.display_name,
            full.from_.address,
        )
    assert flatten(lazy.to) == flatten(full.to)
    assert flatten(lazy.cc) == flatten(full.cc)
    assert flatten(lazy.bcc) == flatten(full.bcc)
    assert flatten(lazy.reply_to) == flatten(full.reply_to)


@pytest.mark.parametrize("path", FIXTURES, ids=IDS)
def test__the_warning_list_is_identical_to_full_mode(path: str):
    # This is what lets `strict=True` mean the same thing in both modes. It holds
    # because the one attachment-level repair the parse can report -- a
    # quoted-printable escape passed through as text -- is found by scanning the
    # *encoded* bytes, which lazy mode still does.
    full, lazy = _both(path)

    def rendered(warnings):
        return [(w.kind, w.part_path, w.detail) for w in warnings]

    assert rendered(lazy.warnings) == rendered(full.warnings)


@pytest.mark.parametrize("path", FIXTURES, ids=IDS)
def test__encoded_size_agrees_with_metadata_mode(path: str):
    # Same name, same number: the two lazy-ish modes must not disagree about how
    # big a part is on the wire.
    raw = _read(path)
    lazy = parse_email(raw, mode="lazy")
    meta = parse_email(raw, mode="metadata")

    sizes = [(a.filename, a.encoded_size) for a in lazy.attachments]
    assert sizes == [(a.filename, a.encoded_size) for a in meta.attachments]


def test__a_str_payload_parses_like_its_bytes(attachment_message: str):
    from_str = parse_email(attachment_message, mode="lazy")
    from_bytes = parse_email(attachment_message.encode("utf-8"), mode="lazy")

    assert [a.content for a in from_str.attachments] == [
        a.content for a in from_bytes.attachments
    ]


# --- the cache ----------------------------------------------------------------


def test__repeated_access_returns_the_same_object(attachment_message: str):
    mail = parse_email(attachment_message, mode="lazy")
    attachment = mail.attachments[0]

    first = attachment.content

    assert attachment.content is first
    assert attachment.content is first


def test__the_attachment_list_hands_back_the_same_objects(attachment_message: str):
    # Reading `attachments` builds a new list, but of the same parts. If it built
    # new parts, each read would get a fresh cache and nothing would ever be
    # cached -- so this is what makes the test above mean anything.
    mail = parse_email(attachment_message, mode="lazy")

    assert mail.attachments[0] is mail.attachments[0]

    first = mail.attachments[0].content
    assert mail.attachments[0].content is first


def test__content_is_bytes(attachment_message: str):
    mail = parse_email(attachment_message, mode="lazy")

    for attachment in mail.attachments:
        assert isinstance(attachment.content, bytes)


def test__attachments_outlive_the_payload(attachment_message: str):
    # `bytes` payloads are borrowed rather than copied (#96), so a mode that
    # retained a view into the caller's buffer would be a use-after-free waiting
    # for a garbage collection. Lazy mode copies each part's encoded bytes; this
    # is what says so.
    payload = attachment_message.encode("utf-8")
    expected = [a.content for a in parse_email(payload).attachments]

    attachments = list(parse_email(payload, mode="lazy").attachments)
    del payload

    assert [a.content for a in attachments] == expected


# --- laziness -----------------------------------------------------------------


def test__nothing_is_decoded_by_the_parse(large_message: str):
    mail = parse_email(large_message.encode("utf-8"), mode="lazy")

    assert mail.attachments, "fixture must contain attachments"
    assert not any(a.is_decoded for a in mail.attachments)


def test__only_the_attachment_read_is_decoded(large_message: str):
    # The criterion #97 asks for, as an assertion rather than a timing proxy:
    # reading one attachment must leave every other one undecoded.
    mail = parse_email(large_message.encode("utf-8"), mode="lazy")
    attachments = list(mail.attachments)
    assert len(attachments) >= 2, "fixture must contain several attachments"

    _ = attachments[0].content

    assert attachments[0].is_decoded
    assert not any(a.is_decoded for a in attachments[1:])


def test__is_decoded_starts_false_and_stays_true(attachment_message: str):
    attachment = parse_email(attachment_message, mode="lazy").attachments[0]

    assert attachment.is_decoded is False
    _ = attachment.content
    assert attachment.is_decoded is True
    _ = attachment.content
    assert attachment.is_decoded is True


# --- thread safety ------------------------------------------------------------


def test__concurrent_first_access_is_safe_and_shares_one_object(large_message: str):
    # The shared-state hazard the free-threading audit flagged, exercised: the
    # GIL is released for the decode, so several threads really are inside it at
    # once. All of them must come back with the *same* object -- the cell is
    # published once and every caller returns what it holds -- and none of them
    # may fail.
    mail = parse_email(large_message.encode("utf-8"), mode="lazy")
    attachment = max(mail.attachments, key=lambda a: a.encoded_size)
    expected = max(parse_email(large_message).attachments, key=lambda a: len(a.content))

    workers = 16
    start = threading.Barrier(workers)
    seen: list[bytes] = []
    failures: list[BaseException] = []
    lock = threading.Lock()

    def hammer():
        try:
            start.wait()
            for _ in range(20):
                content = attachment.content
                with lock:
                    seen.append(content)
        except BaseException as exc:  # noqa: BLE001 - reported, not swallowed
            with lock:
                failures.append(exc)

    threads = [threading.Thread(target=hammer) for _ in range(workers)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    assert not failures, failures
    assert len(seen) == workers * 20
    assert all(content is seen[0] for content in seen), "the cache handed out copies"
    assert seen[0] == expected.content


def test__concurrent_access_across_attachments_is_safe(large_message: str):
    mail = parse_email(large_message.encode("utf-8"), mode="lazy")
    attachments = list(mail.attachments)
    expected = [len(a.content) for a in parse_email(large_message).attachments]

    workers = 8
    start = threading.Barrier(workers)
    failures: list[BaseException] = []

    def hammer():
        try:
            start.wait()
            for _ in range(10):
                for attachment in attachments:
                    assert len(attachment.content) in expected
        except BaseException as exc:  # noqa: BLE001 - reported, not swallowed
            failures.append(exc)

    threads = [threading.Thread(target=hammer) for _ in range(workers)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    assert not failures, failures
    assert all(a.is_decoded for a in attachments)


# --- where a decode error lands -----------------------------------------------


def test__a_broken_attachment_fails_the_full_parse():
    with pytest.raises(DecodeError):
        parse_email(BROKEN_ATTACHMENT)


def test__a_broken_attachment_parses_in_lazy_mode_and_fails_on_access():
    # A real difference in behaviour, pinned rather than left to be discovered:
    # the failure moves from the parse to the attribute, because that is where
    # the decode moved. The rest of the message stays readable, which is usually
    # the more useful of the two.
    mail = parse_email(BROKEN_ATTACHMENT, mode="lazy")

    assert mail.subject == "broken"
    assert len(mail.attachments) == 1

    with pytest.raises(DecodeError):
        _ = mail.attachments[0].content


def test__a_failed_decode_is_not_cached_as_a_value():
    attachment = parse_email(BROKEN_ATTACHMENT, mode="lazy").attachments[0]

    with pytest.raises(DecodeError):
        _ = attachment.content

    # Still undecoded, and still raising: a failure must not be published as if
    # it were a result.
    assert attachment.is_decoded is False
    with pytest.raises(DecodeError):
        _ = attachment.content


# --- strict mode --------------------------------------------------------------


def test__strict_mode_accepts_lazy_mode_on_a_clean_message():
    # Lazy mode reads every body, so it can honour `strict=True` -- unlike
    # metadata mode, which cannot see the bodies at all.
    mail = parse_email(CLEAN, mode="lazy", strict=True)

    assert isinstance(mail, PyLazyMail)
    assert mail.warnings == []


def test__strict_mode_rejects_a_lossy_parse_in_lazy_mode():
    with pytest.raises(DecodeError) as raised:
        parse_email(BAD_DATE, mode="lazy", strict=True)

    assert "strict mode" in str(raised.value)


def test__strict_mode_still_refuses_metadata_mode():
    with pytest.raises(ValueError) as raised:
        parse_email(BAD_DATE, mode="metadata", strict=True)

    # The message must name the modes that do work, or the restriction reads as
    # an oversight.
    assert "lazy" in str(raised.value)


def test__lazy_mode_records_warnings_without_strict():
    mail = parse_email(BAD_DATE, mode="lazy")

    assert [w.kind for w in mail.warnings] == ["date-unparseable"]


# --- the surface --------------------------------------------------------------


def test__the_lazy_attachment_surface_is_frozen(attachment_message: str):
    mail = parse_email(attachment_message, mode="lazy")

    assert mail.attachments, "fixture must contain attachments"
    for attachment in mail.attachments:
        public = {name for name in dir(attachment) if not name.startswith("_")}
        assert public == EXPECTED_LAZY_ATTACHMENT_ATTRS


def test__the_default_mode_is_unchanged(valid_message: str):
    assert isinstance(parse_email(valid_message), PyMail)
    assert isinstance(parse_email(valid_message, mode="full"), PyMail)
    assert isinstance(parse_email(valid_message, mode="lazy"), PyLazyMail)


def test__mode_is_still_keyword_only(valid_message: str):
    with pytest.raises(TypeError):
        parse_email(valid_message, "lazy")


def test__an_unknown_mode_names_lazy(valid_message: str):
    with pytest.raises(ValueError) as raised:
        parse_email(valid_message, mode="deferred")

    assert "lazy" in str(raised.value)
