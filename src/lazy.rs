//! `mode="lazy"`: bodies decoded, attachments deferred to first access (#233).

use std::sync::{Arc, OnceLock};

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDateTime, PyDict, PyList};

use crate::convert::{addresses, date_parsed, decode_into, Headers};
use crate::flat::{ParseWarning, PyAddress};
use crate::mail_parser;
use crate::payload::{Payload, Pinned, RetainedBytes};

/// A non-body part whose bytes are decoded on first access and cached (#97).
///
/// Returned in `attachments` by `parse_email(payload, mode="lazy")`. The fields
/// of `PyAttachment` plus `encoded_size` and `is_decoded`, with `content` a
/// property that does the work rather than a value the parse already paid for.
///
/// Memory: the part points into the payload it was parsed from and keeps it
/// alive, so holding one attachment of a large message holds the message (#239).
/// `content` is a decoded copy, so a caller who wants the bytes and not the
/// message reads it and drops the attachment.
///
/// A new type rather than making `PyAttachment.content` lazy. Changing what an
/// existing attribute costs -- and when it raises -- is a change to a shipped
/// contract, and #104 batches those into one API-v2 window; adding a type is not
/// a breaking change and needs no window. It is the same reasoning that gave
/// metadata mode its own attachment type instead of widening `content` to
/// `bytes | None`.
#[pyclass(frozen, skip_from_py_object)]
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
    /// Where the part sits in the message, still encoded, and the buffer that
    /// keeps those bytes alive (#239). Offsets rather than a copy: the bytes are
    /// already in the payload the caller passed in, and duplicating them is what
    /// deferring the decode is meant to avoid.
    raw: RetainedBytes,
    /// The decoded bytes, published exactly once.
    ///
    /// `OnceLock<Py<PyBytes>>` rather than `OnceLock<Vec<u8>>` so that repeated
    /// access returns the *same* Python object rather than an equal copy of it.
    /// That is both what a cache should mean -- `a.content is a.content` -- and
    /// cheaper, since the second read allocates nothing at all.
    ///
    /// Published once, but *decoded* possibly twice: the decode deliberately
    /// happens outside `get_or_init`, so two threads racing on a first access can
    /// both do the work and one result is dropped. That is the right trade. The
    /// alternative -- decoding inside the initialiser -- holds the cell's lock
    /// across a `detach`, which is the shape that deadlocks: one thread waiting
    /// on the lock while holding the GIL, the other waiting on the GIL while
    /// holding the lock. Wasted CPU on a contended first read is cheaper than
    /// that, and every reader still sees one object.
    content: OnceLock<Py<PyBytes>>,
}

#[pymethods]
impl PyLazyAttachment {
    /// This part's transfer-decoded bytes, decoded on first access and cached.
    ///
    /// Every later read returns the same `bytes` object, so keeping a reference
    /// and re-reading the attribute cost the same thing.
    ///
    /// Two threads reading this for the first time at once may both decode; they
    /// will still see one object. See the note on the field.
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
    pub(crate) fn from_lazy(attachment: mail_parser::LazyAttachment, base: &Arc<Pinned>) -> Self {
        PyLazyAttachment {
            mimetype: attachment.mimetype,
            filename: attachment.filename,
            content_id: attachment.content_id,
            disposition: attachment.disposition,
            encoded_size: attachment.encoded_size,
            raw: RetainedBytes::new(base, attachment.raw),
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
    /// attachments overlap rather than serialise. The slice is a range of a buffer
    /// this object holds an `Arc` to, and that buffer is either an immutable
    /// Python object or a `Vec` nothing else can reach, so nothing can move
    /// underneath it while it is detached.
    ///
    /// `#[cold]` and out of line: it runs at most once per attachment, and the
    /// hot path through the getter above is the cached one.
    #[cold]
    #[inline(never)]
    pub(crate) fn decode<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        decode_into(py, &self.content, self.raw.bytes())
    }
}

/// A parsed message whose attachment content is decoded on demand (#97).
///
/// Returned by `parse_email(payload, mode="lazy")`. Everything except
/// `attachments` is what `PyMail` carries, with the same meaning -- including
/// `warnings`, which is the same list the full parse produces, because lazy mode
/// decodes every body part and finds every repair the full parse finds. That is
/// what lets `strict=True` mean the same thing here.
#[pyclass(frozen, skip_from_py_object)]
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
    pub(crate) headers: Headers,
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
        self.headers.to_dict(py)
    }

    /// The `Date` header as an aware `datetime`, or `None` if unparseable.
    #[getter]
    fn date_parsed<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDateTime>>> {
        date_parsed(py, &self.date)
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
    pub(crate) fn from_lazy(
        py: Python<'_>,
        mail: mail_parser::LazyMail,
        payload: Payload,
    ) -> PyResult<Self> {
        // The pin is built here, from this message's own payload and its own
        // repair, so a batch gets one per slot rather than one shared across the
        // batch -- reading one attachment out of message 3 must not keep messages
        // 1 and 2 alive.
        let base = Pinned::new(payload, mail.repaired);
        let attachments = mail
            .attachments
            .into_iter()
            .map(|part| Py::new(py, PyLazyAttachment::from_lazy(part, &base)))
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
            headers: Headers::new(mail.headers),
        })
    }
}
