//! Reading a caller's payload, and keeping it alive for as long as something
//! points into it (#233).

use std::sync::Arc;

use pyo3::exceptions;
use pyo3::prelude::*;
use pyo3::pybacked::{PyBackedBytes, PyBackedStr};
use pyo3::types::{PyBytes, PyString};

use crate::mail_parser;

/// A caller's payload, ready to be read with the GIL released.
///
/// Both variants borrow (#96, #226). Holding a `PyBackedBytes` or a `PyBackedStr`
/// keeps the Python object alive and hands out its buffer directly, and reading it
/// detached is sound because `bytes` and `str` are both immutable -- nothing can
/// change or free the buffer underneath us while we hold the reference.
///
/// The copy this avoids was not a rounding error at batch sizes. `parse_many`
/// duplicated every payload before parsing any of them, so a batch of ten
/// thousand one-megabyte messages needed ten gigabytes of copies *in addition to*
/// the originals the caller still held.
///
/// `str` was the last holdout, on the premise that the limited API has no UTF-8
/// buffer to borrow. That is not true for the ABI this crate builds: `abi3-py311`
/// sets the `Py_3_10` cfg, under which `PyString::to_str` is the zero-copy
/// `PyUnicode_AsUTF8AndSize` -- the call this function already made one line before
/// the copy -- and `PyBackedStr` wraps exactly that borrow in a `Send + Sync`
/// handle. An ASCII `str` hands over its internal buffer with no encoding step at
/// all; a non-ASCII one makes CPython build the UTF-8 form once and cache it on the
/// object, which is the same cost `to_str` already paid.
pub(crate) enum Payload {
    Bytes(PyBackedBytes),
    Str(PyBackedStr),
}

impl AsRef<[u8]> for Payload {
    fn as_ref(&self) -> &[u8] {
        match self {
            Payload::Bytes(bytes) => bytes.as_ref(),
            Payload::Str(text) => text.as_bytes(),
        }
    }
}

/// Interpret a Python object as a byte buffer for parsing.
///
/// Accepts `bytes` (used as-is) or `str` (read as its UTF-8 bytes; ASCII is
/// unchanged because ASCII == its own UTF-8, and non-ASCII code points round-trip
/// correctly instead of being truncated to their low byte). Any other type raises
/// Python `TypeError`, as does a `str` holding a lone surrogate -- it has no UTF-8
/// form, so there is nothing to borrow and nothing to parse.
pub(crate) fn payload_to_bytes(payload: &Py<PyAny>, py: Python<'_>) -> PyResult<Payload> {
    let obj = payload.bind(py);

    if let Ok(bytes) = obj.cast::<PyBytes>() {
        return Ok(Payload::Bytes(PyBackedBytes::from(bytes.clone())));
    }

    if let Ok(text) = obj.cast::<PyString>() {
        if let Ok(text) = PyBackedStr::try_from(text.clone()) {
            return Ok(Payload::Str(text));
        }
    }

    Err(PyErr::new::<exceptions::PyTypeError, _>(
        "The argument cannot be interpreted as bytes.",
    ))
}

/// The buffer a deferred part's byte range indexes, kept alive for exactly as
/// long as some part still points into it (#239).
///
/// This is the memory contract lazy mode now has, and it is a real change: a
/// result that borrows its bytes pins the message it was parsed from. Holding one
/// attachment of a 100 MB mail holds the 100 MB. That is the right default here
/// -- the alternative is the copy this mode exists to avoid, and the caller who
/// wants the bytes without the payload can ask for `content`, which is a copy by
/// construction, and drop the attachment.
///
/// Both fields are needed because the buffer the parse ran over is not always the
/// one the caller passed: a message whose header block was never terminated is
/// rebuilt first (#150), and the ranges then index the rebuild. Whichever it is,
/// its address is stable across the move into this struct -- a `Vec` owns a heap
/// allocation, and PyO3's backed types point into a Python object -- which is what
/// makes it sound to take the ranges during the parse and build the pin after it.
pub(crate) struct Pinned {
    payload: Payload,
    repaired: Option<Vec<u8>>,
}

impl Pinned {
    pub(crate) fn new(payload: Payload, repaired: Option<Vec<u8>>) -> Arc<Self> {
        let before = payload.as_ref().as_ptr();
        let pinned = Pinned { payload, repaired };
        debug_assert!(
            std::ptr::eq(pinned.payload.as_ref().as_ptr(), before),
            "moving the payload moved the bytes it points at, so ranges taken \
             during the parse no longer index it"
        );
        Arc::new(pinned)
    }

    /// The buffer the parse ran over, which is the one the ranges index.
    fn base(&self) -> &[u8] {
        match &self.repaired {
            Some(repaired) => repaired,
            None => self.payload.as_ref(),
        }
    }
}

/// One deferred part's bytes: where they are, and what keeps them there.
///
/// `Arc` rather than one handle on the message: a part can outlive the
/// `PyLazyMail` or the tree node that produced it, because Python hands out
/// references and a caller can keep the one attachment they wanted. Refcounting
/// the buffer is what makes "keep this attachment, drop everything else" mean the
/// payload is released when the last of them goes.
///
/// An enum rather than a pin plus a `Retained`, so that the part that owns its
/// bytes holds no pin at all. A leaf inside a `message/rfc822` node is the only
/// one that does, and its bytes came from decoding that node rather than from the
/// payload -- pinning the payload for it would keep a whole message alive on
/// behalf of bytes that are not in it.
pub(crate) enum RetainedBytes {
    /// A range of a buffer this part keeps alive.
    In(Arc<Pinned>, std::ops::Range<usize>),
    /// Bytes of its own, for a part that was in no caller buffer.
    Owned(Vec<u8>),
}

impl RetainedBytes {
    pub(crate) fn new(base: &Arc<Pinned>, at: mail_parser::Retained) -> Self {
        match at {
            mail_parser::Retained::Range(range) => RetainedBytes::In(Arc::clone(base), range),
            mail_parser::Retained::Owned(bytes) => RetainedBytes::Owned(bytes),
        }
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        match self {
            RetainedBytes::In(base, range) => &base.base()[range.clone()],
            RetainedBytes::Owned(bytes) => bytes,
        }
    }
}
