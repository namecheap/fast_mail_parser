//! The MIME tree in its three modes: decoded, deferred and described (#233).

use std::sync::{Arc, OnceLock};

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};

use crate::convert::{children_list, decode_into, Headers};
use crate::mail_parser;
use crate::payload::{Pinned, RetainedBytes};

/// One node of a message's MIME tree, with the structure intact (#99).
///
/// `PyMail` is a flattened projection of this -- bodies in one list, attachments
/// in another, containers dropped. Every flattening loses something: which
/// `text/html` part corresponds to which `text/plain` sibling, whether a node was
/// `multipart/alternative` or `multipart/mixed`, where a bounce's inner message
/// begins. Use `parse_email` when the convenience projection is what you want and
/// this when the shape matters.
#[pyclass(frozen, skip_from_py_object)]
pub struct PyMimePart {
    /// The part's media type: `"multipart/alternative"`, `"text/plain"`, ...
    #[pyo3(get)]
    pub content_type: String,
    /// Stored as ordered pairs, exposed through the getter below (#157).
    pub(crate) headers: Headers,
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
    /// The `bytes` handed to Python for `content`, built on first read and then
    /// shared, so `part.content is part.content` here as it is on the lazy
    /// types. For a container the cell is simply never filled.
    content_py: OnceLock<Py<PyBytes>>,
    pub children: Vec<Py<PyMimePart>>,
}

#[pymethods]
impl PyMimePart {
    /// This part's headers, every value kept, keys in wire order.
    #[getter]
    fn headers<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        self.headers.to_dict(py)
    }

    /// Transfer-decoded bytes of a leaf, or `None` for a `multipart/*` container.
    ///
    /// A container's body is its children with boundaries between them, so
    /// returning it would hand back the same bytes twice.
    #[getter]
    fn content<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.content.as_ref().map(|bytes| {
            self.content_py
                .get_or_init(|| PyBytes::new(py, bytes.as_slice()).unbind())
                .bind(py)
                .clone()
        })
    }

    /// The parts nested directly inside this one, in message order.
    #[getter]
    fn children<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        children_list(py, &self.children)
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
    pub(crate) fn from_part(py: Python<'_>, part: mail_parser::MimePart) -> PyResult<Self> {
        let children = part
            .children
            .into_iter()
            .map(|child| Py::new(py, Self::from_part(py, child)?))
            .collect::<PyResult<Vec<_>>>()?;

        Ok(PyMimePart {
            content_type: part.content_type,
            headers: Headers::new(part.headers),
            filename: part.filename,
            content_id: part.content_id,
            disposition: part.disposition,
            is_message: part.is_message,
            content: part.body,
            content_py: OnceLock::new(),
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
#[pyclass(frozen, skip_from_py_object)]
pub struct PyMimePartMetadata {
    #[pyo3(get)]
    pub content_type: String,
    pub(crate) headers: Headers,
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
        self.headers.to_dict(py)
    }

    /// The parts nested directly inside this one, in message order.
    #[getter]
    fn children<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        children_list(py, &self.children)
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
/// Memory: a leaf points at itself where it sits in the caller's payload, so the
/// tree costs its headers and not its bytes -- but it pins that payload for as
/// long as any node of it is alive (#239). A leaf below a `message/rfc822` node
/// is the exception and holds a copy, because the bytes it was parsed from were
/// produced by decoding that node and exist in no caller buffer.
#[pyclass(frozen, skip_from_py_object)]
pub struct PyLazyMimePart {
    #[pyo3(get)]
    pub content_type: String,
    pub(crate) headers: Headers,
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
    raw: Option<RetainedBytes>,
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
        self.headers.to_dict(py)
    }

    /// The parts nested directly inside this one, in message order.
    #[getter]
    fn children<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        children_list(py, &self.children)
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
        self.decode(py, raw.bytes()).map(Some)
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
    pub(crate) fn decode<'py>(&self, py: Python<'py>, raw: &[u8]) -> PyResult<Bound<'py, PyBytes>> {
        decode_into(py, &self.content, raw)
    }
}

/// Build the two deferred node types from one core tree.
///
/// Two `#[inline(never)]` recursions rather than one generic, because the two
/// differ in what they do with the body and in nothing else, and a generic over
/// that would be two instantiations of the same code with an extra layer to read.
/// Both are cold by construction: no flat path reaches either.
#[inline(never)]
pub(crate) fn metadata_node(
    py: Python<'_>,
    node: mail_parser::TreeNode,
) -> PyResult<PyMimePartMetadata> {
    let children = node
        .children
        .into_iter()
        .map(|child| Py::new(py, metadata_node(py, child)?))
        .collect::<PyResult<Vec<_>>>()?;

    Ok(PyMimePartMetadata {
        content_type: node.content_type,
        headers: Headers::new(node.headers),
        filename: node.filename,
        content_id: node.content_id,
        disposition: node.disposition,
        is_message: node.is_message,
        encoded_size: node.body.encoded_size(),
        children,
    })
}

#[inline(never)]
pub(crate) fn lazy_node(
    py: Python<'_>,
    node: mail_parser::TreeNode,
    base: &Arc<Pinned>,
) -> PyResult<PyLazyMimePart> {
    let children = node
        .children
        .into_iter()
        .map(|child| Py::new(py, lazy_node(py, child, base)?))
        .collect::<PyResult<Vec<_>>>()?;

    // A `message/rfc822` body was decoded to reach the children below it, so it
    // arrives already decoded and is published rather than thrown away: the cell
    // is filled here and `is_decoded` is true from the start.
    let (encoded_size, raw, content) = match node.body {
        mail_parser::NodeBody::Container => (None, None, OnceLock::new()),
        mail_parser::NodeBody::Undecoded { encoded_size, raw } => (
            Some(encoded_size),
            raw.map(|at| RetainedBytes::new(base, at)),
            OnceLock::new(),
        ),
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
        headers: Headers::new(node.headers),
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
