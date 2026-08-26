"""Differential compatibility suite: fast_mail_parser vs the stdlib `email`.

Every fixture is parsed by both this library and `email` (policy=default), and
the two are compared on the surface they both model: subject, body text,
attachments, headers, addresses, and the date instant.

The point is not that the two agree everywhere -- it is that **every** place they
disagree is accounted for. A mismatch is only tolerated if it appears in
`DIVERGENCES` with a reason, so a new, unexplained difference fails this test.
That makes the stdlib a standing oracle for the correctness backlog rather than a
one-off comparison, and keeps docs/compatibility.md honest: each row there is a
key in that table.

`STALE` guards the other direction -- a divergence declared here but no longer
observed means the table is describing the past, so that fails too.
"""

import email
import email.policy
import glob
import os

import pytest

from fast_mail_parser import parse_email

_DATA = os.path.join(os.path.dirname(__file__), "data")

# The generated RFC corpus plus the hand-written fixtures, which are closer to
# real-world mail and so likelier to surface a genuine divergence.
# invalid_message.eml is excluded, but NOT because it fails to parse -- despite
# the name, both parsers accept it. It is a real-world message malformed such
# that the two disagree on the body and the header set, which is a documented
# divergence to classify rather than a fixture to compare blindly.
FIXTURES = sorted(glob.glob(os.path.join(_DATA, "rfc", "*.eml"))) + sorted(
    path
    for path in glob.glob(os.path.join(_DATA, "*.eml"))
    if os.path.basename(path) != "invalid_message.eml"
)

# (fixture, dimension) -> why the two legitimately differ.
#
# Populated from observed behaviour; every entry is a documented, deliberate
# difference, not a to-do. Anything not listed here must match.
DIVERGENCES: dict[tuple[str, str], str] = {
    # fast_mail_parser is the more correct of the two here. RFC 6532 permits raw
    # UTF-8 in header values; `email` with policy=default surrogate-escapes those
    # bytes instead of decoding them, so `.addresses` yields lone surrogates
    # ('\udcd0\udc9e...') where this library yields 'Отправитель'.
    ("rfc6532_utf8_headers", "from"): (
        "stdlib surrogate-escapes raw UTF-8 (RFC 6532) in address headers; "
        "this library decodes it"
    ),
    # `headers` exposes header values as they appeared, only unfolded and
    # RFC 2047-decoded. The stdlib additionally *normalises* structured headers,
    # so a Date carrying an RFC 5322 comment loses it: '... +0200 (CEST)' becomes
    # '... +0200'. Use `date_parsed` for an interpreted value.
    ("attachment_message", "headers"): (
        "stdlib strips RFC 5322 comments from structured headers "
        "(Date '+0200 (CEST)' -> '+0200'); this library returns the raw value"
    ),
    # mailparse re-emits an address header's display name quoted and without the
    # space before the angle bracket: 'Example Sender <a@b>' comes back as
    # '"Example Sender"<a@b>'. Semantically equivalent per RFC 5322, textually
    # not. Prefer the typed `from_`/`to` fields over parsing `headers` yourself.
    ("large_message", "headers"): (
        "mailparse re-serialises address-header display names as "
        '\'"Name"<addr>\'; the stdlib preserves the original spelling'
    ),
    # The stdlib's text content manager normalises line endings to LF. This
    # library returns the body as it appeared on the wire, so a message
    # transmitted with CRLF keeps CRLF.
    ("valid_message", "text_plain"): (
        "stdlib normalises body line endings to LF; this library preserves the "
        "wire form (CRLF)"
    ),
    ("valid_message", "text_html"): (
        "stdlib normalises body line endings to LF; this library preserves the "
        "wire form (CRLF)"
    ),
    # Both unfold folded headers, but differently: RFC 5322 folding whitespace is
    # semantically a single space, which is what this library emits. The stdlib
    # keeps the original run of spaces from the continuation lines.
    ("valid_message", "headers"): (
        "on unfolding, this library collapses folding whitespace to one space; "
        "the stdlib preserves the original run"
    ),
}


def _name(path: str) -> str:
    return os.path.splitext(os.path.basename(path))[0]


def _stdlib_view(raw: bytes) -> dict[str, object]:
    message = email.message_from_bytes(raw, policy=email.policy.default)

    plain: list[str] = []
    html: list[str] = []
    attachments: list[tuple[str, str, bytes]] = []

    for part in message.walk():
        if part.get_content_maintype() == "multipart":
            continue
        content_type = part.get_content_type()
        # RFC 2183: an explicit `attachment` disposition means the part is a
        # file whatever its media type. This mirrors what the parser does.
        is_body = (
            part.get_content_disposition() != "attachment"
            and content_type in ("text/plain", "text/html")
        )
        if is_body:
            (plain if content_type == "text/plain" else html).append(
                part.get_content()
            )
        else:
            payload = part.get_payload(decode=True) or b""
            attachments.append((content_type, part.get_filename() or "", payload))

    return {
        "subject": str(message["Subject"] or ""),
        "text_plain": tuple(plain),
        "text_html": tuple(html),
        "attachments": sorted(attachments),
        "headers": sorted((k, str(v)) for k, v in message.items()),
        "from": _stdlib_addresses(message, "From")[:1],
        "to": _stdlib_addresses(message, "To"),
        "cc": _stdlib_addresses(message, "Cc"),
        "date": _stdlib_date(message),
    }


def _stdlib_addresses(message, key: str) -> list[tuple[str | None, str]]:
    """Mailboxes for one address header. `.addresses` already flattens groups."""
    header = message[key]
    if header is None:
        return []
    try:
        return [
            (address.display_name or None, address.addr_spec)
            for address in header.addresses
        ]
    except (AttributeError, ValueError):
        # A header the stdlib could not parse structurally.
        return []


def _stdlib_date(message) -> float | None:
    header = message["Date"]
    if header is None:
        return None
    try:
        parsed = header.datetime
    except (AttributeError, ValueError):
        return None
    return parsed.timestamp() if parsed is not None else None


def _fmp_view(raw: bytes) -> dict[str, object]:
    mail = parse_email(raw)
    return {
        "subject": mail.subject,
        "text_plain": tuple(mail.text_plain),
        "text_html": tuple(mail.text_html),
        "attachments": sorted(
            (a.mimetype, a.filename, a.content) for a in mail.attachments
        ),
        "headers": sorted(
            (name, value) for name, values in mail.headers.items() for value in values
        ),
        "from": (
            [(mail.from_.display_name, mail.from_.address)] if mail.from_ else []
        ),
        "to": [(a.display_name, a.address) for a in mail.to],
        "cc": [(a.display_name, a.address) for a in mail.cc],
        "date": (
            mail.date_parsed.timestamp() if mail.date_parsed is not None else None
        ),
    }


DIMENSIONS = (
    "subject",
    "text_plain",
    "text_html",
    "attachments",
    "headers",
    "from",
    "to",
    "cc",
    "date",
)


def _render(value: object) -> str:
    text = repr(value)
    return text if len(text) <= 300 else text[:300] + "..."


def test__every_divergence_from_the_stdlib_is_accounted_for():
    unexplained: list[str] = []
    observed: set[tuple[str, str]] = set()

    for path in FIXTURES:
        name = _name(path)
        raw = open(path, "rb").read()
        stdlib, fmp = _stdlib_view(raw), _fmp_view(raw)

        for dimension in DIMENSIONS:
            if stdlib[dimension] == fmp[dimension]:
                continue
            observed.add((name, dimension))
            if (name, dimension) in DIVERGENCES:
                continue
            unexplained.append(
                f"\n{name} / {dimension}\n"
                f"    stdlib: {_render(stdlib[dimension])}\n"
                f"    fmp:    {_render(fmp[dimension])}"
            )

    stale = sorted(set(DIVERGENCES) - observed)

    report = []
    if unexplained:
        report.append(
            f"{len(unexplained)} unexplained divergence(s) from the stdlib. "
            "Fix the parser, or add an entry to DIVERGENCES explaining why the "
            "difference is intended (and add the row to docs/compatibility.md):"
            + "".join(unexplained)
        )
    if stale:
        report.append(
            "\nDIVERGENCES entries that no longer occur (remove them, and the "
            f"matching docs/compatibility.md rows): {stale}"
        )
    if report:
        pytest.fail("\n".join(report))


@pytest.mark.parametrize("path", FIXTURES, ids=_name)
def test__both_parsers_accept_every_fixture(path: str):
    # A fixture that only one side can parse is itself a compatibility finding.
    raw = open(path, "rb").read()

    assert _stdlib_view(raw) is not None
    assert _fmp_view(raw) is not None


def test__corpus_is_not_empty():
    assert len(FIXTURES) >= 10, f"expected the RFC corpus, found {len(FIXTURES)}"
