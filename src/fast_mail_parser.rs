//! PyO3 binding layer for the `fast_mail_parser` extension module.
//!
//! The crate intentionally keeps two parallel data models:
//!
//! - [`mail_parser`] is a **PyO3-free core**: `Mail`/`Attachment` are plain Rust
//!   types that hold the parsed message. Because they have no Python dependency,
//!   the parsing logic can be exercised and tested independently of any Python
//!   runtime.
//! - This module is the **PyO3 binding layer**: [`PyMail`]/[`PyAttachment`] wrap
//!   the core types and expose them to Python, converting Rust values into Python
//!   objects (e.g. `Vec<u8>` -> `bytes`).
//!
//! Keeping the split decouples the parsing logic from the Python bindings: the
//! core stays portable and unit-testable, while everything PyO3-specific lives
//! here.

mod mail_parser;

use mailparse::MailParseError;
use pyo3::prelude::*;
use pyo3::pybacked::PyBackedBytes;
use pyo3::types::{PyBytes, PyDateTime, PyDict, PyList, PyString, PyTzInfo};
use pyo3::{create_exception, exceptions, wrap_pyfunction};
use std::num::NonZeroUsize;

create_exception!(fast_mail_parser, ParseError, exceptions::PyException);

// Subtypes of ParseError, so an existing `except ParseError` keeps catching
// everything while callers that care can distinguish the failure. The
// distinction is actionable: a header-section failure usually means the input is
// not an email at all, a structure failure means it is hostile or truncated, and
// a decode failure means one part's transfer encoding is broken while the rest
// of the message may still be worth looking at.
create_exception!(
    fast_mail_parser,
    HeaderParseError,
    ParseError,
    "The header section could not be parsed."
);
create_exception!(
    fast_mail_parser,
    MimeStructureError,
    ParseError,
    "The MIME structure is malformed, or a resource cap was exceeded."
);
create_exception!(
    fast_mail_parser,
    DecodeError,
    ParseError,
    "A part's Content-Transfer-Encoding could not be decoded."
);

/// Classify a parse failure and build the matching Python exception.
///
/// Classification is by `MailParseError` variant, which is exact rather than a
/// heuristic: in mailparse 0.16, transfer-decoding (`body.rs`) only ever yields
/// the three decode variants, and `Generic` is produced solely by the
/// header-parsing paths -- plus the two caps this crate originates itself, which
/// are matched by their named constants.
///
/// Deliberately done here rather than by threading a typed error through the
/// parser: an earlier attempt at that widened the `Result` carried by the
/// per-part loop and the recursive traversal and cost ~30% throughput, which the
/// benchmark gate caught. The classification is cold, so it belongs on the cold
/// side of the boundary.
fn to_py_err(error: MailParseError) -> PyErr {
    let message = format!("Message parsing error: {error}");
    match error {
        MailParseError::Base64DecodeError(_)
        | MailParseError::QuotedPrintableDecodeError(_)
        | MailParseError::EncodingError(_) => DecodeError::new_err(message),
        MailParseError::Generic(detail)
            if detail == mail_parser::ERR_INPUT_TOO_LARGE
                || detail == mail_parser::ERR_MIME_DEPTH =>
        {
            MimeStructureError::new_err(message)
        }
        MailParseError::Generic(_) => HeaderParseError::new_err(message),
    }
}

/// Extract a readable message from a panic payload.
///
/// `panic!` with a literal yields a `&str`; with a format string, a `String`.
/// Anything else is a `panic_any` of some other type, which this crate never
/// does but a dependency could.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<&str>() {
        message
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.as_str()
    } else {
        "unknown panic payload"
    }
}

/// Run a parse and convert a panic into a `ParseError` (#102).
///
/// PyO3 already catches panics at the boundary, so one does not abort the
/// process the way #102 assumed -- it raises `pyo3_runtime.PanicException`. The
/// operational risk is real anyway, for a different reason: `PanicException`
/// derives from `BaseException`, so the `except Exception` in a mail pipeline
/// does not catch it, and a single crafted message takes the worker down.
///
/// A parser fed attacker-controlled bytes should fail like a parser. Raising
/// `ParseError` puts a panic in the same `except` clause as every other
/// unparseable message, and keeping the payload in the message means the bug
/// stays diagnosable rather than swallowed -- the default panic hook has also
/// already written the panic and its location to stderr by this point.
///
/// This is a backstop, not a licence: a panic reaching here is a bug in this
/// crate, and the error says so.
///
/// `inline(always)` is load-bearing, not decoration. This is generic, so every
/// entry point instantiates it, and once there were three the instantiation
/// wrapping `parse_email` stopped being inlined -- which turns the parse body
/// into an opaque call behind unwind edges and cost 24% on large messages while
/// the code responsible was never executed (#99).
#[inline(always)]
fn catch_panics<T>(operation: impl FnOnce() -> PyResult<T>) -> PyResult<T> {
    // `AssertUnwindSafe`: the closure touches Python state, which is not
    // `UnwindSafe`, but nothing observes that state after a panic -- the only
    // thing built here is an error value.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(result) => result,
        Err(payload) => Err(ParseError::new_err(format!(
            "internal parser panic: {} (this is a bug in fast_mail_parser; \
             please report it with the input that triggered it)",
            panic_message(payload.as_ref())
        ))),
    }
}

/// A non-body part described but not decoded, from `mode="metadata"` (#97).
///
/// The same fields as `PyAttachment` minus `content`, plus `encoded_size`. It is
/// a separate type rather than a `PyAttachment` with `content = None`, so that
/// `PyAttachment.content` stays `bytes` for every caller who never asked for this
/// mode -- widening it to `bytes | None` would have broken every `mypy --strict`
/// consumer of the default path.
#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PyAttachmentMetadata {
    #[pyo3(get)]
    pub mimetype: String,
    #[pyo3(get)]
    pub filename: String,
    #[pyo3(get)]
    pub content_id: Option<String>,
    #[pyo3(get)]
    pub disposition: Option<String>,
    /// Bytes this part occupies in the message, **before** transfer-decoding.
    ///
    /// Named for what it is. A bare `size` would be read as the decoded size,
    /// which metadata mode cannot know without doing the decode it exists to
    /// skip: base64 inflates by about a third. In full mode the decoded size is
    /// `len(content)`.
    #[pyo3(get)]
    pub encoded_size: usize,
}

impl PyAttachmentMetadata {
    fn from_metadata(attachment: mail_parser::AttachmentMetadata) -> Self {
        PyAttachmentMetadata {
            mimetype: attachment.mimetype,
            filename: attachment.filename,
            content_id: attachment.content_id,
            disposition: attachment.disposition,
            encoded_size: attachment.encoded_size,
        }
    }
}

/// What a message says about itself, without decoding what it carries (#97).
///
/// Returned by `parse_email(payload, mode="metadata")`. Headers, subject, date
/// and addresses are identical to full mode; attachments are described but not
/// decoded.
///
/// It has no `text_plain`/`text_html` on purpose. #97 proposed empty lists, and
/// an empty list cannot be told apart from "this message has no text part" -- a
/// triage sweep counting bodyless messages would count all of them, which is the
/// same class of silent-wrong-answer as #150. A missing attribute fails loudly.
/// For structure without decoding, `parse_email_tree` is the API that keeps it.
#[pyclass(skip_from_py_object)]
pub struct PyMailMetadata {
    #[pyo3(get)]
    pub subject: String,
    #[pyo3(get)]
    pub date: String,
    #[pyo3(get)]
    pub from_: Option<PyAddress>,
    #[pyo3(get)]
    pub to: Vec<PyAddress>,
    #[pyo3(get)]
    pub cc: Vec<PyAddress>,
    #[pyo3(get)]
    pub bcc: Vec<PyAddress>,
    #[pyo3(get)]
    pub reply_to: Vec<PyAddress>,
    #[pyo3(get)]
    pub attachments: Vec<PyAttachmentMetadata>,
    pub headers: Vec<(String, Vec<String>)>,
}

#[pymethods]
impl PyMailMetadata {
    /// Headers, every value kept, keys in wire order -- as in `PyMail` (#157).
    #[getter]
    fn headers<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        for (name, values) in &self.headers {
            dict.set_item(name, values)?;
        }
        Ok(dict)
    }

    /// The `Date` header as an aware `datetime`, or `None` if unparseable.
    ///
    /// Present here because sorting or bucketing a sweep by date is most of what
    /// metadata mode is for.
    #[getter]
    fn date_parsed<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDateTime>>> {
        let Some(epoch) = mail_parser::parse_date_epoch(&self.date) else {
            return Ok(None);
        };
        let utc = PyTzInfo::utc(py)?;
        PyDateTime::from_timestamp(py, epoch as f64, Some(&utc)).map(Some)
    }

    fn __repr__(&self) -> String {
        format!(
            "<PyMailMetadata {:?} attachments={}>",
            self.subject,
            self.attachments.len()
        )
    }
}

/// Convert an address list for the Python layer.
///
/// Also keeps the conversions below off rustfmt's `chain_width`, which is 60 and
/// which `metadata.to.into_iter().map(...).collect()` exceeds by two characters.
fn addresses(list: Vec<mail_parser::Address>) -> Vec<PyAddress> {
    list.into_iter().map(PyAddress::from_address).collect()
}

impl PyMailMetadata {
    #[inline(never)]
    fn from_metadata(metadata: mail_parser::MailMetadata) -> Self {
        PyMailMetadata {
            subject: metadata.subject,
            date: metadata.date,
            from_: metadata.from_.map(PyAddress::from_address),
            to: addresses(metadata.to),
            cc: addresses(metadata.cc),
            bcc: addresses(metadata.bcc),
            reply_to: addresses(metadata.reply_to),
            attachments: metadata
                .attachments
                .into_iter()
                .map(PyAttachmentMetadata::from_metadata)
                .collect(),
            headers: metadata.headers,
        }
    }
}

/// One node of a message's MIME tree, with the structure intact (#99).
///
/// `PyMail` is a flattened projection of this -- bodies in one list, attachments
/// in another, containers dropped. Every flattening loses something: which
/// `text/html` part corresponds to which `text/plain` sibling, whether a node was
/// `multipart/alternative` or `multipart/mixed`, where a bounce's inner message
/// begins. Use `parse_email` when the convenience projection is what you want and
/// this when the shape matters.
#[pyclass(skip_from_py_object)]
pub struct PyMimePart {
    /// The part's media type: `"multipart/alternative"`, `"text/plain"`, ...
    #[pyo3(get)]
    pub content_type: String,
    /// Stored as ordered pairs, exposed through the getter below (#157).
    pub headers: Vec<(String, Vec<String>)>,
    #[pyo3(get)]
    pub filename: String,
    /// The part's `Content-ID` with angle brackets stripped, or `None`.
    #[pyo3(get)]
    pub content_id: Option<String>,
    /// The part's raw `Content-Disposition` token, or `None` when it declares no
    /// such header. `None` and `"inline"` are distinct statements.
    #[pyo3(get)]
    pub disposition: Option<String>,
    /// True for `message/rfc822`. The embedded message's own root is this part's
    /// single child, so a bounce's headers are reachable rather than opaque.
    #[pyo3(get)]
    pub is_message: bool,
    pub content: Option<Vec<u8>>,
    pub children: Vec<Py<PyMimePart>>,
}

#[pymethods]
impl PyMimePart {
    /// This part's headers, every value kept, keys in wire order.
    #[getter]
    fn headers<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        for (name, values) in &self.headers {
            dict.set_item(name, values)?;
        }
        Ok(dict)
    }

    /// Transfer-decoded bytes of a leaf, or `None` for a `multipart/*` container.
    ///
    /// A container's body is its children with boundaries between them, so
    /// returning it would hand back the same bytes twice.
    #[getter]
    fn content<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.content
            .as_ref()
            .map(|bytes| PyBytes::new(py, bytes.as_slice()))
    }

    /// The parts nested directly inside this one, in message order.
    #[getter]
    fn children<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        PyList::new(py, self.children.iter().map(|child| child.clone_ref(py)))
    }

    fn __repr__(&self) -> String {
        format!(
            "<PyMimePart {} children={}>",
            self.content_type,
            self.children.len()
        )
    }
}

impl PyMimePart {
    /// Cold by construction, and marked so: it recurses, it returns a large
    /// struct, and `parse_email` never reaches it.
    #[inline(never)]
    fn from_part(py: Python<'_>, part: mail_parser::MimePart) -> PyResult<Self> {
        let children = part
            .children
            .into_iter()
            .map(|child| Py::new(py, Self::from_part(py, child)?))
            .collect::<PyResult<Vec<_>>>()?;

        Ok(PyMimePart {
            content_type: part.content_type,
            headers: part.headers,
            filename: part.filename,
            content_id: part.content_id,
            disposition: part.disposition,
            is_message: part.is_message,
            content: part.content,
            children,
        })
    }
}

/// One mailbox from an address header, exposed to Python.
#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PyAddress {
    /// The display name, or `None` when the header carries a bare address.
    ///
    /// RFC 2047 encoded-words are decoded, so a non-ASCII name arrives readable.
    #[pyo3(get)]
    pub display_name: Option<String>,
    /// The `addr-spec` -- the `local@domain` part, without angle brackets.
    #[pyo3(get)]
    pub address: String,
}

impl PyAddress {
    pub(crate) fn from_address(address: mail_parser::Address) -> Self {
        PyAddress {
            display_name: address.display_name,
            address: address.address,
        }
    }
}

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PyAttachment {
    #[pyo3(get)]
    pub mimetype: String,
    pub content: Vec<u8>,
    #[pyo3(get)]
    pub filename: String,
    /// The part's `Content-ID` with angle brackets stripped, or `None`.
    ///
    /// RFC 2392 `cid:` URLs in an HTML body reference this bracket-less form, so
    /// resolving inline images is a lookup keyed on this value.
    #[pyo3(get)]
    pub content_id: Option<String>,
    /// The part's raw `Content-Disposition` token -- typically `"inline"` or
    /// `"attachment"` -- or `None` when the part declares no such header.
    ///
    /// `None` and `"inline"` are reported distinctly: an absent header is not
    /// the same statement as an explicit `inline`.
    #[pyo3(get)]
    pub disposition: Option<String>,
}

#[pymethods]
impl PyAttachment {
    #[getter]
    fn content<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.content.as_slice())
    }
}

impl PyAttachment {
    pub(crate) fn from_attachment(attachment: mail_parser::Attachment) -> Self {
        PyAttachment {
            mimetype: attachment.mimetype,
            content: attachment.content,
            filename: attachment.filename,
            content_id: attachment.content_id,
            disposition: attachment.disposition,
        }
    }
}

/// A parsed email message exposed to Python.
///
/// Body parts and [`attachments`](Self::attachments) are disjoint; `multipart/*`
/// container nodes appear in neither.
#[pyclass]
pub struct PyMail {
    #[pyo3(get)]
    pub subject: String,
    #[pyo3(get)]
    pub text_plain: Vec<String>,
    #[pyo3(get)]
    pub text_html: Vec<String>,
    #[pyo3(get)]
    pub date: String,
    /// The `From` mailbox, or `None` when the header is absent or unparseable.
    ///
    /// Named `from_` because `from` is a Python keyword.
    #[pyo3(get)]
    pub from_: Option<PyAddress>,
    /// `To` recipients. RFC 5322 groups are flattened to their members.
    #[pyo3(get)]
    pub to: Vec<PyAddress>,
    /// `Cc` recipients, flattened as `to` is.
    #[pyo3(get)]
    pub cc: Vec<PyAddress>,
    /// `Bcc` recipients, flattened as `to` is. Usually empty on received mail.
    #[pyo3(get)]
    pub bcc: Vec<PyAddress>,
    /// `Reply-To` mailboxes, flattened as `to` is.
    #[pyo3(get)]
    pub reply_to: Vec<PyAddress>,
    /// The message's non-body parts: real attachments and inline resources.
    ///
    /// Per RFC 2183 a part is body text -- and so absent here -- when it is
    /// `text/plain` or `text/html` and is not marked `Content-Disposition:
    /// attachment`. `multipart/*` container nodes are MIME structure and are not
    /// reported. `filename` may still be empty, which is normal for an inline
    /// image referenced only by `Content-ID`.
    #[pyo3(get)]
    pub attachments: Vec<PyAttachment>,
    /// Stored as ordered pairs, not a map: the key order is the point (#157).
    /// Exposed through the `headers` getter below.
    pub headers: Vec<(String, Vec<String>)>,
}

#[pymethods]
impl PyMail {
    /// `date` parsed to a timezone-aware `datetime`, or `None`.
    ///
    /// Computed on access rather than at parse time: most callers never read it,
    /// and building a Python object for every parsed message would charge them
    /// all for a field they do not use.
    ///
    /// The value is UTC. mailparse resolves the header's offset to an epoch, so
    /// the instant is exact while the original offset is not retained -- read
    /// `date` for that. An unparseable header yields `None`, leaving `date`
    /// intact as the raw string.
    /// All values of every header, keyed by name, in the order the names first
    /// appeared in the message.
    ///
    /// Built on access rather than stored as a dict. Python dicts preserve
    /// insertion order, so inserting in wire order is what makes the ordering
    /// observable -- and the previous `HashMap` field, converted per access,
    /// produced a different order every time (#157).
    #[getter]
    fn headers<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        for (name, values) in &self.headers {
            dict.set_item(name, values)?;
        }
        Ok(dict)
    }

    #[getter]
    fn date_parsed<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDateTime>>> {
        let Some(epoch) = mail_parser::parse_date_epoch(&self.date) else {
            return Ok(None);
        };
        let utc = PyTzInfo::utc(py)?;
        PyDateTime::from_timestamp(py, epoch as f64, Some(&utc)).map(Some)
    }
}

impl PyMail {
    pub(crate) fn from_mail(mail: mail_parser::Mail) -> Self {
        Self {
            subject: mail.subject,
            text_plain: mail.text_plain,
            text_html: mail.text_html,
            date: mail.date,
            from_: mail.from_.map(PyAddress::from_address),
            to: mail.to.into_iter().map(PyAddress::from_address).collect(),
            cc: mail.cc.into_iter().map(PyAddress::from_address).collect(),
            bcc: mail.bcc.into_iter().map(PyAddress::from_address).collect(),
            reply_to: mail
                .reply_to
                .into_iter()
                .map(PyAddress::from_address)
                .collect(),
            attachments: mail
                .attachments
                .into_iter()
                .map(PyAttachment::from_attachment)
                .collect(),
            headers: mail.headers,
        }
    }
}

/// Interpret a Python object as a byte buffer for parsing.
///
/// Accepts `bytes` (used as-is) or `str` (decoded as its UTF-8 bytes; ASCII is
/// unchanged because ASCII == its own UTF-8, and non-ASCII code points round-trip
/// correctly instead of being truncated to their low byte). Any other type raises
/// Python `TypeError`.
/// A caller's payload, ready to be read with the GIL released.
///
/// `bytes` is borrowed rather than copied (#96). Holding a `PyBackedBytes` keeps
/// the Python object alive and hands out its buffer directly, and reading it
/// detached is sound because `bytes` is immutable -- nothing can change or free
/// it underneath us while we hold the reference.
///
/// The copy this avoids was not a rounding error at batch sizes. `parse_many`
/// duplicated every payload before parsing any of them, so a batch of ten
/// thousand one-megabyte messages needed ten gigabytes of copies *in addition to*
/// the originals the caller still held.
///
/// `str` is a different case and still gets copied. Under the limited API,
/// obtaining UTF-8 from a `str` means asking CPython to encode it, which
/// allocates -- there is no buffer to borrow. Callers who care about throughput
/// should pass `bytes`, which is also what reading a mail file or socket gives
/// them.
enum Payload {
    Borrowed(PyBackedBytes),
    Owned(Vec<u8>),
}

impl AsRef<[u8]> for Payload {
    fn as_ref(&self) -> &[u8] {
        match self {
            Payload::Borrowed(bytes) => bytes.as_ref(),
            Payload::Owned(bytes) => bytes.as_slice(),
        }
    }
}

fn payload_to_bytes(payload: &Py<PyAny>, py: Python<'_>) -> PyResult<Payload> {
    let obj = payload.bind(py);

    if let Ok(bytes) = obj.cast::<PyBytes>() {
        return Ok(Payload::Borrowed(PyBackedBytes::from(bytes.clone())));
    }

    if let Ok(text) = obj.cast::<PyString>() {
        if let Ok(text) = text.to_str() {
            return Ok(Payload::Owned(text.as_bytes().to_vec()));
        }
    }

    Err(PyErr::new::<exceptions::PyTypeError, _>(
        "The argument cannot be interpreted as bytes.",
    ))
}

/// Parse a raw email (`bytes` or `str`) into a [`PyMail`].
///
/// Raises `ParseError`, or more precisely one of its subtypes:
/// `HeaderParseError`, `MimeStructureError` or `DecodeError`.
#[pyfunction]
#[pyo3(signature = (payload, *, mode = "full"))]
pub fn parse_email(py: Python<'_>, payload: Py<PyAny>, mode: &str) -> PyResult<Py<PyAny>> {
    // The default path is kept as close to what it was before the mode existed
    // as possible: one comparison, then the same closure. Everything else lives
    // behind an `inline(never)` boundary below.
    //
    // Not a stylistic preference. Putting the three-arm match -- with a `format!`
    // in one arm, which drags in the formatting machinery -- inside the closure
    // that `catch_panics` inlines cost the hot path 30%, for the second time in
    // one day (the first was #180). Code that never runs is not free here.
    if mode == "full" {
        return catch_panics(|| Ok(Py::new(py, parse_email_inner(py, payload)?)?.into_any()));
    }

    parse_email_other_mode(py, payload, mode)
}

#[inline(never)]
fn parse_email_other_mode(py: Python<'_>, payload: Py<PyAny>, mode: &str) -> PyResult<Py<PyAny>> {
    match mode {
        "metadata" => catch_panics(|| parse_email_metadata_mode(py, payload)),
        other => Err(unknown_mode(other)),
    }
}

#[cold]
#[inline(never)]
fn unknown_mode(mode: &str) -> PyErr {
    exceptions::PyValueError::new_err(format!(
        "mode must be \"full\" or \"metadata\", not {mode:?}"
    ))
}

/// Cold, and marked so. `parse_email`'s default path must not pay for this
/// existing: adding cold binding code to this module has already cost the hot
/// path 24% once, through nothing but lost inlining (#99).
#[inline(never)]
fn parse_email_metadata_mode(py: Python<'_>, payload: Py<PyAny>) -> PyResult<Py<PyAny>> {
    let message = payload_to_bytes(&payload, py)?;

    let metadata = py
        .detach(|| mail_parser::parse_email_metadata(message.as_ref()))
        .map_err(to_py_err)?;

    Ok(Py::new(py, PyMailMetadata::from_metadata(metadata))?.into_any())
}

fn parse_email_inner(py: Python<'_>, payload: Py<PyAny>) -> PyResult<PyMail> {
    let message = payload_to_bytes(&payload, py)?;

    // The actual parse is pure Rust and never touches the Python interpreter, so
    // release the GIL (`py.detach`) for its duration. This lets other Python
    // threads -- including other `parse_email` calls -- run concurrently instead
    // of serializing on the GIL, which turns multi-threaded parsing throughput
    // from single-core into multi-core. `message` is an owned copy, so nothing
    // borrows from a Python object while the GIL is released. Errors and the
    // `PyMail` are produced after re-attaching, where the interpreter is needed.
    let mail = py
        .detach(|| mail_parser::parse_email(message.as_ref()))
        .map_err(to_py_err)?;

    Ok(PyMail::from_mail(mail))
}

/// Parse a batch of messages in one call, in parallel, preserving input order.
///
/// Accepts the same `str`/`bytes` inputs as `parse_email`. Each slot of the
/// result is either a `PyMail` or a `ParseError` **instance** -- returned, not
/// raised -- so one malformed message cannot cost the caller the rest of the
/// batch, and inputs zip cleanly to outcomes. `raise_on_error=True` restores
/// fail-fast behaviour for callers who prefer it.
///
/// `threads` caps the worker count; the default is the machine's parallelism.
/// `threads=0` is rejected -- pass `None` to ask for the default.
///
/// Memory: every parsed message is materialised before returning. A batch of ten
/// thousand one-megabyte mails holds essentially all of it decoded at once, so
/// chunk large workloads at the caller.
#[pyfunction]
#[pyo3(signature = (payloads, *, threads = None, raise_on_error = false))]
pub fn parse_many(
    py: Python<'_>,
    payloads: Vec<Py<PyAny>>,
    threads: Option<usize>,
    raise_on_error: bool,
) -> PyResult<Py<PyList>> {
    // A panic fails the whole batch rather than one slot, unlike a parse error.
    // Per-item isolation would need the panic to ride in the core's error type,
    // and `MailParseError::Generic` holds a `&'static str`, so a payload cannot
    // travel that way -- worth revisiting only if a panic is ever actually seen.
    catch_panics(|| parse_many_inner(py, payloads, threads, raise_on_error))
}

fn parse_many_inner(
    py: Python<'_>,
    payloads: Vec<Py<PyAny>>,
    threads: Option<usize>,
    raise_on_error: bool,
) -> PyResult<Py<PyList>> {
    // Resolve every payload *before* releasing the GIL: this touches Python
    // objects, which requires the interpreter. What is held afterwards is a
    // reference to each `bytes` object plus its buffer pointer, not a copy of
    // its contents, so the batch is no longer duplicated in full (#96).
    let messages: Vec<Payload> = payloads
        .iter()
        .map(|payload| payload_to_bytes(payload, py))
        .collect::<PyResult<_>>()?;

    // `threads=0` is meaningless, and silently treating it as "the default"
    // hides a caller bug: `threads=os.cpu_count() - 1` on a one-core machine, or
    // an unset config value, would quietly get full parallelism instead. Reject
    // it and let `None` be the way to ask for the default. `threads` is unsigned,
    // so a negative value already raises OverflowError at conversion.
    let workers = match threads {
        Some(0) => {
            return Err(exceptions::PyValueError::new_err(
                "threads must be at least 1; pass threads=None for the default",
            ));
        }
        other => other.and_then(NonZeroUsize::new),
    };

    // The whole batch parses with the GIL released, so other Python threads keep
    // running for its full duration rather than per message.
    let parsed = py.detach(|| mail_parser::parse_many(&messages, workers));

    let items = PyList::empty(py);
    for result in parsed {
        match result {
            Ok(mail) => items.append(Py::new(py, PyMail::from_mail(mail))?)?,
            Err(error) => {
                let err = to_py_err(error);
                if raise_on_error {
                    return Err(err);
                }
                // The exception object itself, not a raise.
                items.append(err.value(py))?;
            }
        }
    }
    Ok(items.unbind())
}

/// Parse a message into its MIME tree, structure intact.
///
/// Additive: `parse_email` is untouched. Accepts the same `str`/`bytes` payloads
/// and raises the same `ParseError` subtypes, including the recursion and size
/// caps -- an embedded `message/rfc822` counts against the same depth limit as a
/// multipart nest, so an onion of forwards cannot go deeper than a multipart tree.
#[pyfunction]
pub fn parse_email_tree(py: Python<'_>, payload: Py<PyAny>) -> PyResult<PyMimePart> {
    catch_panics(|| parse_email_tree_inner(py, payload))
}

#[inline(never)]
fn parse_email_tree_inner(py: Python<'_>, payload: Py<PyAny>) -> PyResult<PyMimePart> {
    let message = payload_to_bytes(&payload, py)?;

    let tree = py
        .detach(|| mail_parser::parse_email_tree(message.as_ref()))
        .map_err(to_py_err)?;

    PyMimePart::from_part(py, tree)
}

/// Panic on purpose, so the backstop above can be tested.
///
/// Not part of the API: underscore-prefixed, absent from `__all__` and from the
/// type stub, and not re-exported by the package. It exists because an untested
/// backstop is indistinguishable from a missing one, and #102 asks for exactly
/// this kind of trigger. There is no other way to obtain a panic on demand --
/// the parser is not known to panic on any input, which is the whole point.
#[pyfunction]
fn _panic_for_tests() -> PyResult<()> {
    catch_panics(panic_now)
}

fn panic_now() -> PyResult<()> {
    panic!("deliberate panic from _panic_for_tests")
}

#[pymodule]
fn fast_mail_parser(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(parse_email, m)?)?;
    m.add_function(wrap_pyfunction!(parse_many, m)?)?;
    m.add_function(wrap_pyfunction!(parse_email_tree, m)?)?;
    m.add_function(wrap_pyfunction!(_panic_for_tests, m)?)?;
    m.add_class::<PyMail>()?;
    m.add_class::<PyMailMetadata>()?;
    m.add_class::<PyMimePart>()?;
    m.add_class::<PyAttachment>()?;
    m.add_class::<PyAttachmentMetadata>()?;
    m.add_class::<PyAddress>()?;
    m.add("ParseError", py.get_type::<ParseError>())?;
    m.add("HeaderParseError", py.get_type::<HeaderParseError>())?;
    m.add("MimeStructureError", py.get_type::<MimeStructureError>())?;
    m.add("DecodeError", py.get_type::<DecodeError>())?;

    Ok(())
}
