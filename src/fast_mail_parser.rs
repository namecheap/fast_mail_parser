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
use std::sync::OnceLock;

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

/// Build the exception `strict=True` promises for a lossy parse (#100).
///
/// Strict mode is implemented as a check *after* the parse rather than a flag
/// threaded into it. Two reasons. The core never learns about strictness, so the
/// per-part loop keeps the shape #135 measured -- a mode flag tested inside it
/// would be a branch every message pays for a setting almost nobody sets. And
/// the exception is the same one either way: the mapping below is total over the
/// kinds the core emits, so "raise instead of warn" and "warn, then raise"
/// differ only in that the second finishes the parse first and can therefore
/// report how many repairs there were.
///
/// The mapping reuses the #135 hierarchy rather than adding types: a dropped
/// address list is a header-level failure, a resynced header block is a
/// structural one, and a charset fallback or an unreadable date is a value that
/// could not be decoded.
///
/// `#[cold]` and out of line for the same reason as the `warn_*` helpers in the
/// core: it formats, and it must not be inlined into a parse.
#[cold]
#[inline(never)]
fn strict_rejection(warnings: &[ParseWarning]) -> PyErr {
    // Non-empty by construction: the callers check before calling.
    let warning = &warnings[0];
    let location = if warning.part_path.is_empty() {
        "the message".to_owned()
    } else {
        format!("part {}", warning.part_path)
    };
    let message = format!(
        "strict mode rejected a lossy parse: {} on {}: {} \
         ({} warning(s) recorded; parse without strict=True to read them all)",
        warning.kind,
        location,
        warning.detail,
        warnings.len()
    );
    match warning.kind.as_str() {
        mail_parser::KIND_ADDRESS_UNPARSEABLE => HeaderParseError::new_err(message),
        // Structure rather than header parsing: what the defect breaks is the
        // boundary between the header block and the MIME body, and the message
        // parses fine once that is restored -- so `HeaderParseError`, documented
        // as "usually the input is not an email at all", would say the wrong
        // thing about it.
        mail_parser::KIND_UNTERMINATED_HEADERS => MimeStructureError::new_err(message),
        mail_parser::KIND_CHARSET_FALLBACK
        | mail_parser::KIND_DATE_UNPARSEABLE
        | mail_parser::KIND_TRANSFER_DECODE_LOSSY => DecodeError::new_err(message),
        // A kind added to the core without a row above still fails strict mode,
        // just at the base of the hierarchy. Failing open would break the only
        // promise strict mode makes, which is that nothing lossy gets through.
        _ => ParseError::new_err(message),
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

/// One node of a MIME tree, described but not decoded (#202).
///
/// Returned by `parse_email_tree(payload, mode="metadata")`. Everything
/// `PyMimePart` says about the *shape* of a message it says too -- content type,
/// headers, filename, content id, disposition, `is_message`, children -- with
/// `content` replaced by `encoded_size`.
///
/// There is deliberately no `content`, not even `None`. `PyMimePart.content` is
/// `None` for exactly one reason, that the node is a `multipart/*` container, and
/// a mode where `None` also meant "not decoded" would make a leaf with no body
/// indistinguishable from a container. A missing attribute fails loudly instead
/// -- the same choice as `PyMailMetadata`, which omits `text_plain` rather than
/// returning an empty list.
#[pyclass(skip_from_py_object)]
pub struct PyMimePartMetadata {
    #[pyo3(get)]
    pub content_type: String,
    pub headers: Vec<(String, Vec<String>)>,
    #[pyo3(get)]
    pub filename: String,
    #[pyo3(get)]
    pub content_id: Option<String>,
    #[pyo3(get)]
    pub disposition: Option<String>,
    #[pyo3(get)]
    pub is_message: bool,
    /// Bytes this part's body occupies in the message, **before**
    /// transfer-decoding -- as on `PyAttachmentMetadata`. `None` for a
    /// `multipart/*` container, in the same place and with the same meaning as
    /// `PyMimePart.content`'s `None`: a container has no body of its own.
    #[pyo3(get)]
    pub encoded_size: Option<usize>,
    pub children: Vec<Py<PyMimePartMetadata>>,
}

#[pymethods]
impl PyMimePartMetadata {
    /// This part's headers, every value kept, keys in wire order.
    #[getter]
    fn headers<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        for (name, values) in &self.headers {
            dict.set_item(name, values)?;
        }
        Ok(dict)
    }

    /// The parts nested directly inside this one, in message order.
    #[getter]
    fn children<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        PyList::new(py, self.children.iter().map(|child| child.clone_ref(py)))
    }

    fn __repr__(&self) -> String {
        format!(
            "<PyMimePartMetadata {} children={}>",
            self.content_type,
            self.children.len()
        )
    }
}

/// One node of a MIME tree whose bytes are decoded on first access (#202).
///
/// Returned by `parse_email_tree(payload, mode="lazy")`. `PyMimePart` plus
/// `encoded_size` and `is_decoded`, with `content` a property that does the work
/// rather than a value the parse already paid for -- the same relationship
/// `PyLazyAttachment` has to `PyAttachment`, and a new type for the same reason:
/// re-timing an existing attribute, and moving where it raises, is a change to a
/// shipped contract that #104 batches into an API-v2 window.
///
/// Memory: a leaf retains a copy of itself as it sits in the message. For a
/// single-part message that is the whole payload, because the root *is* the leaf
/// -- so this mode is for walking a large multipart message and decoding one part
/// of it, which is what the tree is for, and not for small mail in bulk.
#[pyclass(skip_from_py_object)]
pub struct PyLazyMimePart {
    #[pyo3(get)]
    pub content_type: String,
    pub headers: Vec<(String, Vec<String>)>,
    #[pyo3(get)]
    pub filename: String,
    #[pyo3(get)]
    pub content_id: Option<String>,
    #[pyo3(get)]
    pub disposition: Option<String>,
    #[pyo3(get)]
    pub is_message: bool,
    /// Bytes this part's body occupies before transfer-decoding, or `None` for a
    /// `multipart/*` container -- the same value and name as on
    /// `PyMimePartMetadata` and `PyLazyAttachment`. Choosing which part to decode
    /// must not require decoding any of them, which is what this is for.
    #[pyo3(get)]
    pub encoded_size: Option<usize>,
    /// The part as it sits in the message, still encoded. `None` for a container
    /// and for a `message/rfc822` node, whose bytes are already published below.
    raw: Option<Vec<u8>>,
    /// The decoded bytes, published exactly once -- as on `PyLazyAttachment`, so
    /// that `part.content is part.content`.
    content: OnceLock<Py<PyBytes>>,
    pub children: Vec<Py<PyLazyMimePart>>,
}

#[pymethods]
impl PyLazyMimePart {
    /// This part's headers, every value kept, keys in wire order.
    #[getter]
    fn headers<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        for (name, values) in &self.headers {
            dict.set_item(name, values)?;
        }
        Ok(dict)
    }

    /// The parts nested directly inside this one, in message order.
    #[getter]
    fn children<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        PyList::new(py, self.children.iter().map(|child| child.clone_ref(py)))
    }

    /// Transfer-decoded bytes of a leaf, decoded on first access and cached, or
    /// `None` for a `multipart/*` container.
    ///
    /// `None` means what it means on `PyMimePart`: a container's body is its
    /// children with boundaries between them, so returning it would hand back the
    /// same bytes twice. It never means "not decoded" -- `is_decoded` answers
    /// that, and reading this attribute is what changes the answer.
    ///
    /// Raises `DecodeError` when this part's `Content-Transfer-Encoding` cannot be
    /// decoded, exactly as `PyLazyAttachment.content` does: full mode fails the
    /// whole tree, this mode fails only the part.
    #[getter]
    fn content<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyBytes>>> {
        if let Some(cached) = self.content.get() {
            return Ok(Some(cached.bind(py).clone()));
        }
        let Some(raw) = self.raw.as_ref() else {
            return Ok(None);
        };
        self.decode(py, raw).map(Some)
    }

    /// Whether reading `content` is free.
    ///
    /// True once the part has been decoded, and true from the start for a
    /// container and for a `message/rfc822` node -- neither has a decode pending.
    /// That is the question a caller holding a large tree actually has, and it is
    /// also what makes "a part nobody reads is never decoded" an assertion rather
    /// than a timing argument.
    #[getter]
    fn is_decoded(&self) -> bool {
        self.content.get().is_some() || self.raw.is_none()
    }

    fn __repr__(&self) -> String {
        format!(
            "<PyLazyMimePart {} children={} decoded={}>",
            self.content_type,
            self.children.len(),
            self.is_decoded()
        )
    }
}

impl PyLazyMimePart {
    /// Decode this part and publish the result, exactly as `PyLazyAttachment`
    /// does -- see the long note there for why a race duplicates work rather than
    /// needing a lock, and why the GIL is released for the decode.
    #[cold]
    #[inline(never)]
    fn decode<'py>(&self, py: Python<'py>, raw: &[u8]) -> PyResult<Bound<'py, PyBytes>> {
        let decoded = py
            .detach(|| mail_parser::decode_part(raw))
            .map_err(to_py_err)?;
        let bytes = PyBytes::new(py, decoded.as_slice()).unbind();

        Ok(self.content.get_or_init(|| bytes).bind(py).clone())
    }
}

/// Build the two deferred node types from one core tree.
///
/// Two `#[inline(never)]` recursions rather than one generic, because the two
/// differ in what they do with the body and in nothing else, and a generic over
/// that would be two instantiations of the same code with an extra layer to read.
/// Both are cold by construction: no flat path reaches either.
#[inline(never)]
fn metadata_node(py: Python<'_>, node: mail_parser::TreeNode) -> PyResult<PyMimePartMetadata> {
    let children = node
        .children
        .into_iter()
        .map(|child| Py::new(py, metadata_node(py, child)?))
        .collect::<PyResult<Vec<_>>>()?;

    Ok(PyMimePartMetadata {
        content_type: node.content_type,
        headers: node.headers,
        filename: node.filename,
        content_id: node.content_id,
        disposition: node.disposition,
        is_message: node.is_message,
        encoded_size: node.body.encoded_size(),
        children,
    })
}

#[inline(never)]
fn lazy_node(py: Python<'_>, node: mail_parser::TreeNode) -> PyResult<PyLazyMimePart> {
    let children = node
        .children
        .into_iter()
        .map(|child| Py::new(py, lazy_node(py, child)?))
        .collect::<PyResult<Vec<_>>>()?;

    // A `message/rfc822` body was decoded to reach the children below it, so it
    // arrives already decoded and is published rather than thrown away: the cell
    // is filled here and `is_decoded` is true from the start.
    let (encoded_size, raw, content) = match node.body {
        mail_parser::NodeBody::Container => (None, None, OnceLock::new()),
        mail_parser::NodeBody::Undecoded { encoded_size, raw } => {
            (Some(encoded_size), raw, OnceLock::new())
        }
        mail_parser::NodeBody::Decoded {
            encoded_size,
            content,
        } => {
            let cell = OnceLock::new();
            let bytes = PyBytes::new(py, content.as_slice()).unbind();
            let _ = cell.set(bytes);
            (Some(encoded_size), None, cell)
        }
    };

    Ok(PyLazyMimePart {
        content_type: node.content_type,
        headers: node.headers,
        filename: node.filename,
        content_id: node.content_id,
        disposition: node.disposition,
        is_message: node.is_message,
        encoded_size,
        raw,
        content,
        children,
    })
}

/// One lossy repair a parse performed, reported rather than raised (#100).
///
/// Read-only, three `str` fields, no interior mutability -- the same shape as
/// [`PyAddress`], so the free-threading invariant recorded in the `mail_parser`
/// module still holds.
#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct ParseWarning {
    /// A stable token naming what was repaired: `"charset-fallback"`,
    /// `"address-unparseable"`, `"date-unparseable"`. This is the field to
    /// match on; the set grows as repairs become observable, so treat an
    /// unrecognised kind as "something was repaired" rather than as impossible.
    #[pyo3(get)]
    pub kind: String,
    /// Where the affected part landed in the result -- `"text_plain[0]"`,
    /// `"text_html[1]"` -- or `""` when the warning is about the message as a
    /// whole rather than one part.
    ///
    /// A locator into the returned `PyMail` rather than MIME tree coordinates:
    /// `parse_email` hands back a flat projection, and a coordinate naming
    /// structure it has already discarded would be a locator the caller cannot
    /// resolve. `parse_email_tree` is where tree coordinates belong.
    #[pyo3(get)]
    pub part_path: String,
    /// Prose for whoever reads the log. Deliberately not a matching key: the
    /// wording is free to improve, `kind` is not.
    #[pyo3(get)]
    pub detail: String,
}

#[pymethods]
impl ParseWarning {
    fn __repr__(&self) -> String {
        format!(
            "<ParseWarning {} {:?}: {}>",
            self.kind, self.part_path, self.detail
        )
    }
}

impl ParseWarning {
    /// Cold by construction: a well-formed message never reaches this.
    #[inline(never)]
    fn from_warning(warning: mail_parser::Warning) -> Self {
        ParseWarning {
            kind: warning.kind.to_owned(),
            part_path: warning.part_path,
            detail: warning.detail,
        }
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
    /// Every lossy repair this parse performed, in the order it performed them
    /// (#100).
    ///
    /// **Empty means a pristine parse.** That is the guarantee worth having and
    /// the reason this is a list rather than a log line: a pipeline can treat
    /// `warnings == []` as "nothing here was patched up" and route everything
    /// else to quarantine or review. Best-effort parsing was always the
    /// behaviour; this is what makes it observable.
    ///
    /// Cheap when empty by construction, which is the case that matters: the
    /// core builds a `Vec` that does not allocate until something is pushed, and
    /// every push sits behind a branch well-formed mail does not take.
    #[pyo3(get)]
    pub warnings: Vec<ParseWarning>,
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
            // Empty in the common case, where `collect` allocates nothing.
            warnings: mail
                .warnings
                .into_iter()
                .map(ParseWarning::from_warning)
                .collect(),
        }
    }
}

/// A non-body part whose bytes are decoded on first access and cached (#97).
///
/// Returned in `attachments` by `parse_email(payload, mode="lazy")`. The fields
/// of `PyAttachment` plus `encoded_size` and `is_decoded`, with `content` a
/// property that does the work rather than a value the parse already paid for.
///
/// A new type rather than making `PyAttachment.content` lazy. Changing what an
/// existing attribute costs -- and when it raises -- is a change to a shipped
/// contract, and #104 batches those into one API-v2 window; adding a type is not
/// a breaking change and needs no window. It is the same reasoning that gave
/// metadata mode its own attachment type instead of widening `content` to
/// `bytes | None`.
#[pyclass(skip_from_py_object)]
pub struct PyLazyAttachment {
    #[pyo3(get)]
    pub mimetype: String,
    #[pyo3(get)]
    pub filename: String,
    /// The part's `Content-ID` with angle brackets stripped, or `None`.
    #[pyo3(get)]
    pub content_id: Option<String>,
    /// The part's raw `Content-Disposition` token, or `None` when the part
    /// declares no such header. `None` and `"inline"` are distinct statements.
    #[pyo3(get)]
    pub disposition: Option<String>,
    /// Bytes the part occupies in the message, **before** transfer-decoding --
    /// the same value and the same name as `PyAttachmentMetadata.encoded_size`.
    ///
    /// It is what makes selective extraction possible: choosing which attachment
    /// to decode is exactly the decision this mode exists to serve, and a size
    /// that required a decode to obtain would defeat it.
    #[pyo3(get)]
    pub encoded_size: usize,
    /// The part as it sits in the message, still encoded.
    raw: Vec<u8>,
    /// The decoded bytes, published exactly once.
    ///
    /// `OnceLock<Py<PyBytes>>` rather than `OnceLock<Vec<u8>>` so that repeated
    /// access returns the *same* Python object rather than an equal copy of it.
    /// That is both what a cache should mean -- `a.content is a.content` -- and
    /// cheaper, since the second read allocates nothing at all.
    content: OnceLock<Py<PyBytes>>,
}

#[pymethods]
impl PyLazyAttachment {
    /// This part's transfer-decoded bytes, decoded on first access and cached.
    ///
    /// Every later read returns the same `bytes` object, so keeping a reference
    /// and re-reading the attribute cost the same thing.
    ///
    /// Raises `DecodeError` when the part's `Content-Transfer-Encoding` cannot be
    /// decoded. Full mode raises that from `parse_email`; this mode raises it
    /// from here, because here is where the decode happens. A message with one
    /// broken attachment therefore parses in this mode and fails only on that
    /// attachment, which is usually the more useful of the two behaviours -- and
    /// is the same trade metadata mode makes by never decoding at all.
    #[getter]
    fn content<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        if let Some(cached) = self.content.get() {
            return Ok(cached.bind(py).clone());
        }
        self.decode(py)
    }

    /// Whether `content` has been decoded yet.
    ///
    /// Deliberately public rather than a test hook. It is the only way to observe
    /// that this mode does what it claims, which turns "an attachment nobody
    /// reads is never decoded" from a timing argument into an assertion; and it
    /// answers a real question for a caller holding a large inventory, namely
    /// whether reading `content` is free or is about to cost a decode.
    #[getter]
    fn is_decoded(&self) -> bool {
        self.content.get().is_some()
    }

    fn __repr__(&self) -> String {
        format!(
            "<PyLazyAttachment {} {:?} encoded_size={} decoded={}>",
            self.mimetype,
            self.filename,
            self.encoded_size,
            self.content.get().is_some()
        )
    }
}

impl PyLazyAttachment {
    fn from_lazy(attachment: mail_parser::LazyAttachment) -> Self {
        PyLazyAttachment {
            mimetype: attachment.mimetype,
            filename: attachment.filename,
            content_id: attachment.content_id,
            disposition: attachment.disposition,
            encoded_size: attachment.encoded_size,
            raw: attachment.raw,
            content: OnceLock::new(),
        }
    }

    /// Decode this part, publish the result, and hand back whatever is published
    /// -- which is not necessarily what this call decoded.
    ///
    /// Two threads can arrive here together, and then both decode: `OnceLock`
    /// has no fallible `get_or_try_init` on stable, and initialising through a
    /// closure that cannot fail would mean either panicking on a broken transfer
    /// encoding or caching the failure. Duplicated work under a race is the
    /// cheaper defect, and it is bounded -- the loser's `PyBytes` is dropped and
    /// every caller, winner or loser, returns the object the cell holds. So the
    /// promise callers actually depend on, that `content` is always the same
    /// object, holds without a lock.
    ///
    /// The GIL is released for the decode, so several threads pulling different
    /// attachments overlap rather than serialise. `raw` is owned by this object
    /// and this object is immutable, so nothing can move underneath the slice
    /// while it is detached.
    ///
    /// `#[cold]` and out of line: it runs at most once per attachment, and the
    /// hot path through the getter above is the cached one.
    #[cold]
    #[inline(never)]
    fn decode<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let raw = self.raw.as_slice();
        let decoded = py
            .detach(|| mail_parser::decode_part(raw))
            .map_err(to_py_err)?;
        let bytes = PyBytes::new(py, decoded.as_slice()).unbind();

        Ok(self.content.get_or_init(|| bytes).bind(py).clone())
    }
}

/// A parsed message whose attachment content is decoded on demand (#97).
///
/// Returned by `parse_email(payload, mode="lazy")`. Everything except
/// `attachments` is what `PyMail` carries, with the same meaning -- including
/// `warnings`, which is the same list the full parse produces, because lazy mode
/// decodes every body part and finds every repair the full parse finds. That is
/// what lets `strict=True` mean the same thing here.
#[pyclass(skip_from_py_object)]
pub struct PyLazyMail {
    #[pyo3(get)]
    pub subject: String,
    #[pyo3(get)]
    pub text_plain: Vec<String>,
    #[pyo3(get)]
    pub text_html: Vec<String>,
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
    pub warnings: Vec<ParseWarning>,
    /// Held as Python objects rather than Rust values, which is what makes the
    /// cache mean anything: the same `PyLazyAttachment` has to come back from
    /// every read of this attribute, or each read would hand out a fresh cache
    /// and nothing would ever be cached.
    pub attachments: Vec<Py<PyLazyAttachment>>,
    pub headers: Vec<(String, Vec<String>)>,
}

#[pymethods]
impl PyLazyMail {
    /// The message's non-body parts, in message order, undecoded.
    #[getter]
    fn attachments<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        PyList::new(py, self.attachments.iter().map(|part| part.clone_ref(py)))
    }

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
            "<PyLazyMail {:?} attachments={}>",
            self.subject,
            self.attachments.len()
        )
    }
}

impl PyLazyMail {
    /// Cold by construction, and marked so: `parse_email`'s default path never
    /// reaches this, and cold binding code in this module has already cost the
    /// hot path 24% through nothing but lost inlining (#99).
    #[inline(never)]
    fn from_lazy(py: Python<'_>, mail: mail_parser::LazyMail) -> PyResult<Self> {
        let attachments = mail
            .attachments
            .into_iter()
            .map(|part| Py::new(py, PyLazyAttachment::from_lazy(part)))
            .collect::<PyResult<Vec<_>>>()?;

        Ok(PyLazyMail {
            subject: mail.subject,
            text_plain: mail.text_plain,
            text_html: mail.text_html,
            date: mail.date,
            from_: mail.from_.map(PyAddress::from_address),
            to: addresses(mail.to),
            cc: addresses(mail.cc),
            bcc: addresses(mail.bcc),
            reply_to: addresses(mail.reply_to),
            warnings: mail
                .warnings
                .into_iter()
                .map(ParseWarning::from_warning)
                .collect(),
            attachments,
            headers: mail.headers,
        })
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
///
/// Repairs the parser makes on the way -- a charset label it could not
/// recognise, an address header it could not parse, a header block it had to
/// resync (#150) -- are recorded on `PyMail.warnings` rather than raised, so
/// `warnings == []` is a statement a caller can act on. `strict=True` turns each
/// of them into the matching `ParseError` subtype instead, for validation
/// pipelines that would rather see a failure than a repair. It requires a mode
/// that reads the bodies, so `"full"` or `"lazy"`.
///
/// `mode="lazy"` returns a [`PyLazyMail`]: the bodies decoded as today, and each
/// attachment's content decoded on first access and cached. `mode="metadata"`
/// returns a [`PyMailMetadata`] and decodes nothing at all.
#[pyfunction]
#[pyo3(signature = (payload, *, mode = "full", strict = false))]
pub fn parse_email(
    py: Python<'_>,
    payload: Py<PyAny>,
    mode: &str,
    strict: bool,
) -> PyResult<Py<PyAny>> {
    // `strict` is answered first and then forgotten, so that everything below
    // this line is byte-for-byte the revision before strict mode existed.
    //
    // That is not caution, it is a measurement. Threading the flag through
    // `parse_email_inner` cost **+47% on every entry point and +96% on metadata
    // mode**; routing metadata through a shared slow-path helper still cost +29%
    // and +96%. Neither did any work: `parse_email_tree` and `parse_many`
    // regressed by the same amount with their code untouched. Bisected with
    // dispatched runs against one base, the warning machinery in the core
    // measured +0.9% alone, the `ParseWarning` pyclass +0.3% alone, and the two
    // `#[cold]` helpers +0.6% merely by existing. Only the plumbing was
    // expensive, so there is none.
    if strict {
        return parse_email_strict_mode(py, payload, mode);
    }

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

/// One `catch_panics` call site for both non-default modes, rather than the more
/// readable arm-per-mode. `catch_panics` is generic over its closure, so an arm
/// of its own is an instantiation of its own -- and #99 is the record of a third
/// instantiation stopping the one that wraps `parse_email` from being inlined and
/// costing the hot path 24% while the new code never ran. Adding a mode should
/// not add one.
#[inline(never)]
fn parse_email_other_mode(py: Python<'_>, payload: Py<PyAny>, mode: &str) -> PyResult<Py<PyAny>> {
    let lazy = match mode {
        "lazy" => true,
        "metadata" => false,
        other => return Err(unknown_mode(other)),
    };

    catch_panics(|| {
        if lazy {
            return parse_email_lazy_mode(py, payload);
        }
        parse_email_metadata_mode(py, payload)
    })
}

/// `strict=True`: the same parse, and then a verdict on what it repaired.
///
/// A path of its own rather than a flag on the one above, for the measured
/// reason recorded there. It costs a second `catch_panics` instantiation, which
/// #99 says is not free either -- but that function carries `#[inline(always)]`
/// for exactly this, and an instantiation is cheaper than a hot-path argument.
///
/// `mode="lazy"` honours it too, and it means the same thing there. That mode
/// decodes every body part exactly as full mode does and finds every repair full
/// mode finds -- the one attachment-level repair the parse can report is found by
/// scanning the *encoded* bytes, which lazy mode still does -- so the warning list
/// is the same list. Deferring attachment content changes when a `DecodeError`
/// surfaces, and nothing about what was repaired.
///
/// Metadata mode cannot honour it. It never reads the bodies, so the strongest
/// thing it could say is "nothing in the *headers* was repaired", and a flag
/// meaning something weaker than it says is worse than one that is unavailable --
/// the same reasoning that leaves `text_plain` absent from that mode rather than
/// empty.
///
/// One `catch_panics` call site covers both modes, rather than the more readable
/// branch around two. #99 measured what a further instantiation of that generic
/// can do to the inlining of the one wrapping `parse_email`, and readability is
/// not worth re-testing it for.
#[cold]
#[inline(never)]
fn parse_email_strict_mode(py: Python<'_>, payload: Py<PyAny>, mode: &str) -> PyResult<Py<PyAny>> {
    if mode == "metadata" {
        return Err(strict_needs_decoded_bodies());
    }
    let lazy = mode == "lazy";
    if !lazy && mode != "full" {
        return Err(unknown_mode(mode));
    }

    catch_panics(|| {
        if lazy {
            let mail = parse_lazy_inner(py, payload)?;
            if !mail.warnings.is_empty() {
                return Err(strict_rejection(&mail.warnings));
            }
            return Ok(Py::new(py, mail)?.into_any());
        }
        let mail = parse_email_inner(py, payload)?;
        if !mail.warnings.is_empty() {
            return Err(strict_rejection(&mail.warnings));
        }
        Ok(Py::new(py, mail)?.into_any())
    })
}

#[cold]
#[inline(never)]
fn unknown_mode(mode: &str) -> PyErr {
    exceptions::PyValueError::new_err(format!(
        "mode must be \"full\", \"lazy\" or \"metadata\", not {mode:?}"
    ))
}

#[cold]
#[inline(never)]
fn strict_needs_decoded_bodies() -> PyErr {
    exceptions::PyValueError::new_err(
        "strict=True needs mode=\"full\" or mode=\"lazy\": metadata mode does \
         not read the bodies, so it cannot tell you that nothing in them was \
         repaired",
    )
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

/// Cold, and marked so, for the same reason as the metadata entry point above.
#[inline(never)]
fn parse_email_lazy_mode(py: Python<'_>, payload: Py<PyAny>) -> PyResult<Py<PyAny>> {
    let mail = parse_lazy_inner(py, payload)?;

    Ok(Py::new(py, mail)?.into_any())
}

#[inline(never)]
fn parse_lazy_inner(py: Python<'_>, payload: Py<PyAny>) -> PyResult<PyLazyMail> {
    let message = payload_to_bytes(&payload, py)?;

    // The GIL is released for the parse, as in every other mode. What the parse
    // retains per attachment is a copy of that part's encoded bytes, so nothing
    // borrows from the caller's `bytes` once this returns -- which is what lets
    // the attachments outlive the payload.
    let mail = py
        .detach(|| mail_parser::parse_email_lazy(message.as_ref()))
        .map_err(to_py_err)?;

    PyLazyMail::from_lazy(py, mail)
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
///
/// Warnings ride along per message on each `PyMail.warnings`; `strict=True`
/// turns a lossy parse into that slot's error, exactly as it turns one into a
/// raise for `parse_email`, so the two APIs agree on what "strict" means.
///
/// `mode=` takes the same three values `parse_email` takes and means the same
/// things, so each slot holds a `PyMail`, a `PyLazyMail` or a `PyMailMetadata`.
/// The mode is uniform across the batch -- it picks the slot type through the
/// stub's overloads, which is only sound because one call cannot mix them.
/// `strict=True` with `mode="metadata"` raises `ValueError`, exactly as it does
/// on `parse_email`, and for the same reason: that mode never reads the bodies.
#[pyfunction]
#[pyo3(signature = (
    payloads, *, mode = "full", threads = None, raise_on_error = false, strict = false
))]
pub fn parse_many(
    py: Python<'_>,
    payloads: Vec<Py<PyAny>>,
    mode: &str,
    threads: Option<usize>,
    raise_on_error: bool,
    strict: bool,
) -> PyResult<Py<PyList>> {
    // `mode` is answered first and then forgotten, so everything below this line
    // is byte-for-byte the revision before the mode existed -- the pattern #100,
    // #99 and #180 each arrived at the hard way, and the reason `parse_email`
    // reads the way it does.
    if mode != "full" {
        return parse_many_other_mode(py, payloads, mode, threads, raise_on_error, strict);
    }

    // A panic fails the whole batch rather than one slot, unlike a parse error.
    // Per-item isolation would need the panic to ride in the core's error type,
    // and `MailParseError::Generic` holds a `&'static str`, so a payload cannot
    // travel that way -- worth revisiting only if a panic is ever actually seen.
    let inner = || parse_many_inner(py, payloads, threads, raise_on_error, strict);
    catch_panics(inner)
}

/// One `catch_panics` call site for both non-default batch modes, as
/// `parse_email_other_mode` is for the flat ones (#99).
#[inline(never)]
fn parse_many_other_mode(
    py: Python<'_>,
    payloads: Vec<Py<PyAny>>,
    mode: &str,
    threads: Option<usize>,
    raise_on_error: bool,
    strict: bool,
) -> PyResult<Py<PyList>> {
    let lazy = match mode {
        "lazy" => true,
        "metadata" => {
            // The same rejection, with the same message, as `parse_email`. A
            // batch of metadata cannot promise more about the bodies than one
            // message of it can.
            if strict {
                return Err(strict_needs_decoded_bodies());
            }
            false
        }
        other => return Err(unknown_mode(other)),
    };

    catch_panics(|| {
        if lazy {
            return parse_many_lazy(py, payloads, threads, raise_on_error, strict);
        }
        parse_many_metadata(py, payloads, threads, raise_on_error)
    })
}

/// Turn the `threads` argument into a worker cap, rejecting zero.
///
/// `threads=0` is meaningless, and silently treating it as "the default" hides a
/// caller bug: `threads=os.cpu_count() - 1` on a one-core machine, or an unset
/// config value, would quietly get full parallelism instead. Reject it and let
/// `None` be the way to ask for the default. `threads` is unsigned, so a negative
/// value already raises OverflowError at conversion.
///
/// Out of line and shared by all three modes rather than written three times: the
/// message is part of the API, and three copies of it are three chances for them
/// to stop agreeing. `#[inline(never)]` for the usual reason in this module --
/// the error construction is cold and belongs nowhere near a caller's body.
#[inline(never)]
fn resolve_workers(threads: Option<usize>) -> PyResult<Option<NonZeroUsize>> {
    match threads {
        Some(0) => Err(exceptions::PyValueError::new_err(
            "threads must be at least 1; pass threads=None for the default",
        )),
        other => Ok(other.and_then(NonZeroUsize::new)),
    }
}

fn parse_many_inner(
    py: Python<'_>,
    payloads: Vec<Py<PyAny>>,
    threads: Option<usize>,
    raise_on_error: bool,
    strict: bool,
) -> PyResult<Py<PyList>> {
    // Resolve every payload *before* releasing the GIL: this touches Python
    // objects, which requires the interpreter. What is held afterwards is a
    // reference to each `bytes` object plus its buffer pointer, not a copy of
    // its contents, so the batch is no longer duplicated in full (#96).
    let messages: Vec<Payload> = payloads
        .iter()
        .map(|payload| payload_to_bytes(payload, py))
        .collect::<PyResult<_>>()?;

    let workers = resolve_workers(threads)?;

    // The whole batch parses with the GIL released, so other Python threads keep
    // running for its full duration rather than per message.
    let parsed = py.detach(|| mail_parser::parse_many(&messages, workers));

    let items = PyList::empty(py);
    for result in parsed {
        // Under `strict`, a lossy parse becomes this slot's failure. Folded into
        // the same `Err` arm as a parse error so `raise_on_error` needs no second
        // implementation: one notion of "this slot failed", two ways to reach it.
        let outcome = match result {
            Ok(mail) => {
                let mail = PyMail::from_mail(mail);
                if strict && !mail.warnings.is_empty() {
                    Err(strict_rejection(&mail.warnings))
                } else {
                    Ok(mail)
                }
            }
            Err(error) => Err(to_py_err(error)),
        };
        match outcome {
            Ok(mail) => items.append(Py::new(py, mail)?)?,
            Err(err) => {
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

/// `parse_many(..., mode="metadata")`: headers and an attachment inventory per
/// message, decoding nothing (#202).
///
/// The mailbox sweep the mode was built for. #96 measured the batch API at ~12x
/// on 2000 small messages, and #97 measured metadata mode at ~4x on an
/// attachment-heavy one; before this a caller had to pick one of the two.
///
/// No `strict`: rejected at the boundary above, as on `parse_email`.
///
/// `#[inline(never)]`, like every other cold binding function in this module.
#[inline(never)]
fn parse_many_metadata(
    py: Python<'_>,
    payloads: Vec<Py<PyAny>>,
    threads: Option<usize>,
    raise_on_error: bool,
) -> PyResult<Py<PyList>> {
    // Borrowed, not copied, exactly as in full mode (#96).
    let messages: Vec<Payload> = payloads
        .iter()
        .map(|payload| payload_to_bytes(payload, py))
        .collect::<PyResult<_>>()?;

    let workers = resolve_workers(threads)?;

    let parsed = py.detach(|| {
        mail_parser::parse_many_as(&messages, workers, mail_parser::parse_email_metadata)
    });

    let items = PyList::empty(py);
    for result in parsed {
        match result {
            Ok(metadata) => {
                items.append(Py::new(py, PyMailMetadata::from_metadata(metadata))?)?;
            }
            Err(error) => {
                let err = to_py_err(error);
                if raise_on_error {
                    return Err(err);
                }
                items.append(err.value(py))?;
            }
        }
    }
    Ok(items.unbind())
}

/// `parse_many(..., mode="lazy")`: bodies decoded, attachments deferred (#202).
///
/// `strict=True` means here what it means everywhere else, and is honoured
/// per slot the way full mode honours it: lazy mode finds every repair the full
/// parse finds, so the verdict is the same verdict.
///
/// Worth a caution the single-message mode does not need: this retains the
/// encoded bytes of every attachment in the *whole batch* until the batch is
/// dropped, which is the opposite of what the mode saves on one message. Use it
/// to sweep a batch and pull a few parts out of it, not to hold ten thousand
/// messages' attachments undecoded.
#[inline(never)]
fn parse_many_lazy(
    py: Python<'_>,
    payloads: Vec<Py<PyAny>>,
    threads: Option<usize>,
    raise_on_error: bool,
    strict: bool,
) -> PyResult<Py<PyList>> {
    let messages: Vec<Payload> = payloads
        .iter()
        .map(|payload| payload_to_bytes(payload, py))
        .collect::<PyResult<_>>()?;

    let workers = resolve_workers(threads)?;

    let parsed =
        py.detach(|| mail_parser::parse_many_as(&messages, workers, mail_parser::parse_email_lazy));

    let items = PyList::empty(py);
    for result in parsed {
        // Folded into one `Err` arm as in full mode: one notion of "this slot
        // failed", two ways to reach it.
        let outcome = match result {
            Ok(mail) => {
                let mail = PyLazyMail::from_lazy(py, mail)?;
                if strict && !mail.warnings.is_empty() {
                    Err(strict_rejection(&mail.warnings))
                } else {
                    Ok(mail)
                }
            }
            Err(error) => Err(to_py_err(error)),
        };
        match outcome {
            Ok(mail) => items.append(Py::new(py, mail)?)?,
            Err(err) => {
                if raise_on_error {
                    return Err(err);
                }
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
///
/// No warnings channel, and no `strict`. It does apply the #150 header/body
/// separator repair, so there is one repair it could report; what it has no
/// place to put it is a return type, since `PyMimePart` is a node rather than a
/// message. A tree-shaped channel is also the one that should carry real MIME
/// coordinates, which this traversal has for free and the flat one does not --
/// see `ParseWarning.part_path`. Both are one decision, and not this one.
///
/// `mode="lazy"` returns a [`PyLazyMimePart`] tree, whose leaves decode on first
/// access -- walk a large message, decode the one part you want.
/// `mode="metadata"` returns a [`PyMimePartMetadata`] tree, which decodes nothing
/// and retains nothing. The shape is identical in all three modes; the modes
/// differ only in what a leaf's bytes cost.
#[pyfunction]
#[pyo3(signature = (payload, *, mode = "full"))]
pub fn parse_email_tree(py: Python<'_>, payload: Py<PyAny>, mode: &str) -> PyResult<Py<PyAny>> {
    // `mode` is answered here and then forgotten, and the default arm is one
    // comparison followed by the call this function has always made. Same shape
    // as `parse_email` and for the same measured reason: in this crate a match
    // with a `format!` arm, inlined into the closure `catch_panics` inlines, has
    // cost the parse path 30% while never executing.
    if mode == "full" {
        return catch_panics(|| Ok(Py::new(py, parse_email_tree_inner(py, payload)?)?.into_any()));
    }

    parse_email_tree_other_mode(py, payload, mode)
}

/// One `catch_panics` call site for both deferred tree modes, as
/// `parse_email_other_mode` is for the flat ones -- #99 is the record of a third
/// instantiation of that generic costing the hot path 24% while never running.
#[inline(never)]
fn parse_email_tree_other_mode(
    py: Python<'_>,
    payload: Py<PyAny>,
    mode: &str,
) -> PyResult<Py<PyAny>> {
    let defer = match mode {
        "lazy" => true,
        "metadata" => false,
        other => return Err(unknown_mode(other)),
    };

    catch_panics(|| {
        let message = payload_to_bytes(&payload, py)?;

        let tree = py
            .detach(|| mail_parser::parse_tree_deferred(message.as_ref(), defer))
            .map_err(to_py_err)?;

        if defer {
            return Ok(Py::new(py, lazy_node(py, tree)?)?.into_any());
        }
        Ok(Py::new(py, metadata_node(py, tree)?)?.into_any())
    })
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
    m.add_class::<PyLazyMail>()?;
    m.add_class::<PyMailMetadata>()?;
    m.add_class::<PyMimePart>()?;
    m.add_class::<PyLazyMimePart>()?;
    m.add_class::<PyMimePartMetadata>()?;
    m.add_class::<PyAttachment>()?;
    m.add_class::<PyLazyAttachment>()?;
    m.add_class::<PyAttachmentMetadata>()?;
    m.add_class::<PyAddress>()?;
    m.add_class::<ParseWarning>()?;
    m.add("ParseError", py.get_type::<ParseError>())?;
    m.add("HeaderParseError", py.get_type::<HeaderParseError>())?;
    m.add("MimeStructureError", py.get_type::<MimeStructureError>())?;
    m.add("DecodeError", py.get_type::<DecodeError>())?;

    Ok(())
}
