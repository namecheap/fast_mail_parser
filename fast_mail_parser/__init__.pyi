from collections.abc import Iterator
from datetime import datetime
from typing import Literal, overload

__all__ = [
    "parse_email",
    "parse_email_tree",
    "parse_many",
    "walk",
    "PyMail",
    "PyLazyMail",
    "PyMailMetadata",
    "PyMimePart",
    "PyAttachment",
    "PyLazyAttachment",
    "PyAttachmentMetadata",
    "PyAddress",
    "ParseWarning",
    "ParseError",
    "HeaderParseError",
    "MimeStructureError",
    "DecodeError",
]


class PyMimePart:
    """One node of a message's MIME tree, with the structure intact.

    ``PyMail`` is a flattened projection of this -- bodies in one list,
    attachments in another, containers dropped. Every flattening loses something:
    which ``text/html`` part corresponds to which ``text/plain`` sibling, whether
    a node was ``multipart/alternative`` or ``multipart/mixed``, where a bounce's
    inner message begins.

    ``content`` is the transfer-decoded bytes of a leaf, and ``None`` for a
    ``multipart/*`` container -- whose body is just its children with boundaries
    between them.

    ``is_message`` is ``True`` for ``message/rfc822``: the embedded message's own
    root is this part's single child, so a bounce's headers are reachable rather
    than opaque. Such nesting counts against the same recursion cap as a
    multipart tree.

    ``headers`` keeps every value of every header, keys in the order the names
    first appeared -- the same semantics as ``PyMail.headers``.
    """

    def __init__(
        self,
        content_type: str,
        headers: dict[str, list[str]],
        filename: str,
        content_id: str | None,
        disposition: str | None,
        is_message: bool,
        content: bytes | None,
        children: list[PyMimePart],
    ) -> None:
        self.content_type = content_type
        self.headers = headers
        self.filename = filename
        self.content_id = content_id
        self.disposition = disposition
        self.is_message = is_message
        self.content = content
        self.children = children


class PyAttachmentMetadata:
    """A non-body part described but not decoded, from ``mode="metadata"``.

    The same fields as ``PyAttachment`` minus ``content``, plus
    ``encoded_size`` -- the bytes this part occupies in the message *before*
    transfer-decoding. Named for what it is: metadata mode cannot know the
    decoded size without doing the decode it exists to skip, and base64 inflates
    by about a third. In full mode the decoded size is ``len(content)``.

    It is **not** an upper bound on the decoded size. Base64 decodes smaller, but
    quoted-printable emits a line break as CRLF, so a body of bare LFs decodes
    *larger* than it was encoded. Decoding cannot more than double a part, which
    is the bound the test suite and the fuzz harness assert.

    A separate type rather than a ``PyAttachment`` whose ``content`` is ``None``,
    so that ``PyAttachment.content`` stays ``bytes`` for every caller who never
    asked for this mode.
    """

    def __init__(
        self,
        mimetype: str,
        filename: str,
        content_id: str | None,
        disposition: str | None,
        encoded_size: int,
    ) -> None:
        self.mimetype = mimetype
        self.filename = filename
        self.content_id = content_id
        self.disposition = disposition
        self.encoded_size = encoded_size


class PyMailMetadata:
    """What a message says about itself, without decoding what it carries.

    Returned by ``parse_email(payload, mode="metadata")``. ``subject``, ``date``,
    ``date_parsed``, the address fields and ``headers`` are identical to full
    mode; ``attachments`` are described but not decoded.

    There is deliberately no ``text_plain``/``text_html``. An empty list cannot be
    told apart from "this message has no text part", so a triage sweep counting
    bodyless messages would count all of them; a missing attribute fails loudly
    instead. For structure without decoding, ``parse_email_tree`` keeps it.
    """

    def __init__(
        self,
        subject: str,
        date: str,
        from_: PyAddress | None,
        to: list[PyAddress],
        cc: list[PyAddress],
        bcc: list[PyAddress],
        reply_to: list[PyAddress],
        attachments: list[PyAttachmentMetadata],
        headers: dict[str, list[str]],
    ) -> None:
        self.subject = subject
        self.date = date
        self.from_ = from_
        self.to = to
        self.cc = cc
        self.bcc = bcc
        self.reply_to = reply_to
        self.attachments = attachments
        self.headers = headers

    @property
    def date_parsed(self) -> datetime | None:
        """The ``Date`` header as an aware ``datetime``, or ``None``."""


class PyAddress:
    """One mailbox from an address header.

    ``display_name`` is ``None`` when the header carries a bare address. RFC 2047
    encoded-words are decoded, so a non-ASCII name arrives readable.
    ``address`` is the ``addr-spec`` -- ``local@domain``, without angle brackets.
    """

    def __init__(self, display_name: str | None, address: str) -> None:
        self.display_name = display_name
        self.address = address

class ParseWarning:
    """One lossy repair a parse performed, reported rather than raised.

    ``kind`` is the stable token to match on -- currently
    ``"charset-fallback"``, ``"address-unparseable"`` or
    ``"date-unparseable"``. The set grows as more repairs become observable, so
    treat an unrecognised kind as "something was repaired" rather than as
    impossible.

    ``part_path`` says where the affected part landed in the result
    (``"text_plain[0]"``, ``"text_html[1]"``), or is ``""`` when the warning is
    about the message as a whole. It is a locator into the returned ``PyMail``
    rather than MIME tree coordinates: ``parse_email`` returns a flat
    projection, so a coordinate naming structure it has already dropped would
    be one the caller could not resolve. Use ``parse_email_tree`` when the
    structure is what matters.

    ``detail`` is prose for a log, deliberately not a matching key: the wording
    is free to improve, ``kind`` is not.
    """

    def __init__(self, kind: str, part_path: str, detail: str) -> None:
        self.kind = kind
        self.part_path = part_path
        self.detail = detail

class PyAttachment:
    """A non-body MIME part: a real attachment or an inline resource.

    ``filename`` is taken from the ``Content-Disposition`` ``filename``
    parameter (RFC 2183, including RFC 2231 extended values), falling back to
    the ``Content-Type`` ``name`` parameter. It is ``""`` when the part declares
    neither -- common for inline images referenced only by ``Content-ID``.

    ``content_id`` is the part's ``Content-ID`` with angle brackets stripped, or
    ``None``. RFC 2392 ``cid:`` URLs in an HTML body reference this bracket-less
    form, so resolving inline images is a lookup keyed on this value.

    ``disposition`` is the raw ``Content-Disposition`` token (typically
    ``"inline"`` or ``"attachment"``), or ``None`` when the part declares no such
    header -- an absent header is reported distinctly from an explicit
    ``inline``.
    """

    def __init__(
        self,
        mimetype: str,
        content: bytes,
        filename: str,
        content_id: str | None,
        disposition: str | None,
    ) -> None:
        self.mimetype = mimetype
        self.content = content
        self.filename = filename
        self.content_id = content_id
        self.disposition = disposition


class PyLazyAttachment:
    """A non-body part whose content is decoded on first access, from ``mode="lazy"``.

    The fields of ``PyAttachment`` plus ``encoded_size`` and ``is_decoded``, with
    ``content`` a property that decodes rather than a value the parse already paid
    for. Every read after the first returns **the same** ``bytes`` object.

    ``content`` raises ``DecodeError`` when the part's
    ``Content-Transfer-Encoding`` cannot be decoded. Full mode raises that from
    ``parse_email``; here it is raised on access, because that is where the
    decode happens -- so a message with one broken attachment parses, and fails
    only on that attachment.

    ``encoded_size`` is the bytes the part occupies in the message *before*
    transfer-decoding, exactly as on ``PyAttachmentMetadata``. It is what makes
    selective extraction work: choosing which attachment to decode must not
    require decoding any of them.

    ``is_decoded`` says whether ``content`` has been decoded yet -- so whether
    reading it is free or is about to cost a decode.

    A separate type rather than a lazy ``PyAttachment.content``: changing what an
    existing attribute costs, and when it raises, is a change to a shipped
    contract, and those batch into one API-v2 window. Adding a type is not.
    
    Reading ``content`` from two threads for the first time simultaneously may
    decode twice; both readers still get the same object back. The decode is
    deliberately not held under the cache's lock, because doing so would risk a
    deadlock against the GIL.
    """

    def __init__(
        self,
        mimetype: str,
        filename: str,
        content_id: str | None,
        disposition: str | None,
        encoded_size: int,
    ) -> None:
        self.mimetype = mimetype
        self.filename = filename
        self.content_id = content_id
        self.disposition = disposition
        self.encoded_size = encoded_size

    @property
    def content(self) -> bytes:
        """The transfer-decoded bytes, decoded on first access and cached."""

    @property
    def is_decoded(self) -> bool:
        """Whether ``content`` has been decoded yet."""


class PyLazyMail:
    """A parsed message whose attachment content is decoded on demand.

    Returned by ``parse_email(payload, mode="lazy")``. Everything except
    ``attachments`` is what ``PyMail`` carries, with the same meaning --
    including ``warnings``, which is the same list the full parse produces:
    lazy mode decodes every body part and finds every repair full mode finds. So
    ``strict=True`` means the same thing here as there.

    ``attachments`` are ``PyLazyAttachment`` values, and the same objects on every
    read of the attribute -- which is what makes their caches worth anything.
    """

    def __init__(
        self,
        subject: str,
        text_plain: list[str],
        text_html: list[str],
        date: str,
        from_: PyAddress | None,
        to: list[PyAddress],
        cc: list[PyAddress],
        bcc: list[PyAddress],
        reply_to: list[PyAddress],
        attachments: list[PyLazyAttachment],
        headers: dict[str, list[str]],
        warnings: list[ParseWarning],
    ) -> None:
        self.subject = subject
        self.text_plain = text_plain
        self.text_html = text_html
        self.date = date
        self.from_ = from_
        self.to = to
        self.cc = cc
        self.bcc = bcc
        self.reply_to = reply_to
        self.attachments = attachments
        self.headers = headers
        self.warnings = warnings

    @property
    def date_parsed(self) -> datetime | None:
        """The ``Date`` header as an aware ``datetime``, or ``None``."""


class PyMail:
    """A parsed message.

    ``date_parsed`` is ``date`` resolved to a timezone-aware ``datetime`` in UTC,
    or ``None`` when the header is absent or unparseable. It is computed on
    access, so reading it is the only cost. The instant is exact; the header's
    original offset is not retained -- read ``date`` for that.

    Address headers are parsed into ``PyAddress`` values, taken from the first
    occurrence of each header. RFC 5322 groups (``To: team: a@x, b@x;``) are
    flattened to their members. An address header that does not parse yields an
    empty list (or ``None`` for ``from_``) rather than raising -- a malformed
    ``To:`` must not fail an otherwise good message, and the raw value stays
    available through ``headers``.

    ``headers`` maps each header name to **every** value it appeared with, in
    order, so repeated keys such as ``Received`` or ``DKIM-Signature`` are
    preserved. Single-valued headers are one-element lists:
    ``headers["From"] == ["a@example.com"]``. The keys are in the order the names
    first appeared in the message, stably across parses.

    Body parts and attachments are disjoint. A part is body text -- reaching
    ``text_plain`` or ``text_html`` -- when it is ``text/plain`` or ``text/html``
    and is not marked ``Content-Disposition: attachment``; every other part is an
    attachment. ``multipart/*`` container nodes are MIME structure and appear in
    neither list.

    ``warnings`` lists every lossy repair the parse performed, in order. **An
    empty list means a pristine parse** -- the guarantee a pipeline can act on:
    route ``warnings != []`` to quarantine or manual review rather than
    classifying a message on content that was patched up. Parsing best-effort is
    not new; being able to tell that it happened is.
    """

    def __init__(
        self,
        subject: str,
        text_plain: list[str],
        text_html: list[str],
        date: str,
        from_: PyAddress | None,
        to: list[PyAddress],
        cc: list[PyAddress],
        bcc: list[PyAddress],
        reply_to: list[PyAddress],
        attachments: list[PyAttachment],
        headers: dict[str, list[str]],
        warnings: list[ParseWarning],
    ) -> None:
        self.subject = subject
        self.text_plain = text_plain
        self.text_html = text_html
        self.date = date
        self.from_ = from_
        self.to = to
        self.cc = cc
        self.bcc = bcc
        self.reply_to = reply_to
        self.attachments = attachments
        self.headers = headers
        self.warnings = warnings

    @property
    def date_parsed(self) -> datetime | None: ...


class ParseError(Exception):
    """Base class for every parse failure.

    ``except ParseError`` catches all of the subtypes below, so code written
    against it keeps working.
    """


class HeaderParseError(ParseError):
    """The header section could not be parsed.

    Usually means the input is not an email at all.
    """


class MimeStructureError(ParseError):
    """The MIME structure is malformed, or a resource cap was exceeded.

    Raised for a payload over the 100 MiB input limit and for MIME nesting
    deeper than 256 levels -- both hostile-input guards rather than ordinary
    malformedness.
    """


class DecodeError(ParseError):
    """A part's ``Content-Transfer-Encoding`` could not be decoded.

    For example base64 or quoted-printable that does not decode. The rest of the
    message may still have been well-formed.
    """


@overload
def parse_email(payload: str | bytes, *, strict: bool = False) -> PyMail:
    """Parse raw content of email and return structured datatype.

    A missing ``Subject`` or ``Date`` header yields the empty string ``""``
    (not ``None``) on the returned ``PyMail``.

    ``mode="lazy"`` decodes the bodies as usual and defers each attachment:
    ``PyLazyAttachment.content`` decodes on first access and caches, so an
    attachment nobody reads is never decoded. Returns a ``PyLazyMail``. It trades
    memory for decoding -- the encoded bytes of every attachment are retained,
    and base64 is about 1.33x the size of what it encodes -- so it is for
    selective extraction, not for reading everything anyway.

    ``mode="metadata"`` reads the headers and the attachment inventory without
    transfer-decoding anything, and returns a ``PyMailMetadata``. On an
    attachment-heavy message that is most of the work skipped. The mode picks the
    return type through these overloads, so callers of the default path see no
    change at all.

    ``mode`` must be ``"full"``, ``"lazy"`` or ``"metadata"``; anything else
    raises ``ValueError``. It is keyword-only.

    Raises a ``ParseError`` subtype: ``HeaderParseError``,
    ``MimeStructureError`` or ``DecodeError``. Catch ``ParseError`` to handle
    all of them. Note that ``mode="metadata"`` cannot raise ``DecodeError``,
    because it never decodes, and that ``mode="lazy"`` raises an attachment's
    ``DecodeError`` from ``content`` rather than from here.

    Repairs short of a failure -- an unrecognised charset label, an unparseable
    address header, an unreadable ``Date``, a header block that had to be
    resynced -- are recorded on ``PyMail.warnings`` instead of raising, so
    ``warnings == []`` means nothing was patched up.

    ``strict=True`` raises the matching ``ParseError`` subtype instead of
    recording a warning, for validation pipelines that would rather fail than
    accept a repair. It changes nothing about which messages parse cleanly, and
    it requires a mode that reads the bodies -- ``"full"`` or ``"lazy"``.
    Metadata mode never reads them, so it cannot promise nothing in them was
    repaired; combining the two raises ``ValueError``, and the overloads below
    reject it statically.
    """


@overload
def parse_email(
    payload: str | bytes, *, mode: Literal["full"], strict: bool = False
) -> PyMail: ...


@overload
def parse_email(
    payload: str | bytes, *, mode: Literal["lazy"], strict: bool = False
) -> PyLazyMail: ...


@overload
def parse_email(
    payload: str | bytes, *, mode: Literal["metadata"]
) -> PyMailMetadata: ...


def parse_email_tree(payload: str | bytes) -> PyMimePart:
    """Parse a message into its MIME tree, structure intact.

    Additive: ``parse_email`` is unchanged. Accepts the same payloads and raises
    the same ``ParseError`` subtypes, including the size and recursion caps.

    Use this when the shape of the message matters -- forensics, bounce
    processing, deciding which body belongs to which alternative -- and
    ``parse_email`` when the flat projection is what you want.
    """


def walk(part: PyMimePart) -> Iterator[PyMimePart]:
    """Yield ``part`` and every part beneath it, depth first.

    The same order as the stdlib's ``email.message.Message.walk``. It is a
    generator, so stopping early costs nothing for the rest of the tree.
    """
def parse_many(
    payloads: list[str | bytes],
    *,
    threads: int | None = None,
    raise_on_error: bool = False,
    strict: bool = False,
) -> list[PyMail | ParseError]:
    """Parse a batch of messages in one call, in parallel, in input order.

    Each slot of the result is either a ``PyMail`` or a ``ParseError``
    *instance* -- returned, not raised -- so one malformed message does not cost
    the caller the rest of the batch, and inputs zip cleanly to outcomes::

        for payload, outcome in zip(payloads, parse_many(payloads)):
            if isinstance(outcome, ParseError):
                ...

    Pass ``raise_on_error=True`` to raise the first failure instead.

    Warnings ride along per message on each ``PyMail.warnings``.
    ``strict=True`` makes a lossy parse that slot's failure, the same way it
    makes one a raise for ``parse_email`` -- combined with
    ``raise_on_error=True`` it fails the batch on the first repair.

    ``threads`` caps the worker count; the default is the machine's parallelism.
    ``threads=0`` raises ``ValueError`` -- pass ``None`` for the default. The GIL
    is released for the whole batch rather than per message.

    Every parsed message is materialised before returning, so chunk large
    workloads at the caller.

    Best suited to **many small messages**: the overhead it removes scales with
    the message count, while its one cost -- copying each payload before parsing
    begins -- scales with total bytes. For a few very large messages a Python
    thread pool over ``parse_email`` is currently faster. See the README for
    measured figures.
    """
