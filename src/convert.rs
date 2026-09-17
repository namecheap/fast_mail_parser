//! Conversions the binding layer would otherwise write out once per `#[pyclass]`
//! (#233).
//!
//! The rule this applies is the one the file already stated on `resolve_workers`:
//! *"Out of line and shared by all three modes rather than written three times:
//! the message is part of the API, and three copies of it are three chances for
//! them to stop agreeing."* That is as true of `date_parsed`'s tzinfo, of the
//! decode-once cache, and of what `raise_on_error=False` puts in a failed slot,
//! as it is of a worker count.
//!
//! None of this is on the parse path. The getters run on Python attribute access
//! and the batch helpers run with the GIL re-acquired, after the detached
//! parallel parse has already finished.
//!
//! `headers` is deliberately absent: the six copies that used to exist were
//! replaced in #231 by `Headers::to_dict`, which caches the dict as well as
//! sharing the code, so every getter is already a one-line delegation.

use std::sync::OnceLock;

use pyo3::PyClass;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDateTime, PyList, PyTzInfo};

use crate::{ParseWarning, mail_parser, strict_rejection, to_py_err};

/// The `Date` header as an aware UTC `datetime`, or `None` if it will not parse.
///
/// `None` rather than an error because `date` keeps the raw string either way --
/// what the caller loses is the parsed form, not the value. A message whose Date
/// does not parse says so through `warnings` instead.
pub(crate) fn date_parsed<'py>(
    py: Python<'py>,
    date: &str,
) -> PyResult<Option<Bound<'py, PyDateTime>>> {
    let Some(epoch) = mail_parser::parse_date_epoch(date) else {
        return Ok(None);
    };
    let utc = PyTzInfo::utc(py)?;
    PyDateTime::from_timestamp(py, epoch as f64, Some(&utc)).map(Some)
}

/// A node's children as a list, each a new reference to the same object.
///
/// New references, not copies: `child is node.children[0]` holds, and walking a
/// tree twice hands back the same objects both times.
///
/// A tiny non-recursive generic on purpose. The #99 incident was a *generic
/// wrapping the whole parse body*, which cost 24% by stopping an unrelated
/// function from inlining; this one has nothing under it to stop inlining.
pub(crate) fn children_list<'py, T: PyClass>(
    py: Python<'py>,
    children: &[Py<T>],
) -> PyResult<Bound<'py, PyList>> {
    PyList::new(py, children.iter().map(|child| child.clone_ref(py)))
}

/// Decode one retained part, publish the result in `cell`, and hand back whatever
/// is published -- which is not necessarily what this call decoded.
///
/// Two threads can arrive here together, and then both decode: `OnceLock` has no
/// fallible `get_or_try_init` on stable, and initialising through a closure that
/// cannot fail would mean either panicking on a broken transfer encoding or
/// caching the failure. Duplicated work under a race is the cheaper defect, and
/// it is bounded -- the loser's `PyBytes` is dropped and every caller, winner or
/// loser, returns the object the cell holds. So the promise callers depend on,
/// that `content` is always the same object, holds without a lock.
///
/// The GIL is released for the decode, so several threads pulling different parts
/// overlap rather than serialise. Decoding inside `get_or_init` instead would hold
/// the cell's lock across a `detach`, which is the shape that deadlocks: one
/// thread waiting on the lock while holding the GIL, the other waiting on the GIL
/// while holding the lock.
///
/// `#[cold]` and out of line, as both copies of this were: it runs at most once
/// per part, and the hot path through the getters above it is the cached one.
#[cold]
#[inline(never)]
pub(crate) fn decode_into<'py>(
    py: Python<'py>,
    cell: &OnceLock<Py<PyBytes>>,
    raw: &[u8],
) -> PyResult<Bound<'py, PyBytes>> {
    let decoded = py
        .detach(|| mail_parser::decode_part(raw))
        .map_err(to_py_err)?;
    let bytes = PyBytes::new(py, decoded.as_slice()).unbind();

    Ok(cell.get_or_init(|| bytes).bind(py).clone())
}

/// `strict=True`'s verdict on one parse: a repaired message is a failure.
///
/// One function rather than the four copies of `if strict && !warnings.is_empty()`
/// that existed, because the exception it raises -- which repair it names, how it
/// counts the rest -- is part of the API, and four copies of that are four chances
/// for them to stop agreeing.
pub(crate) fn strict_gate(strict: bool, warnings: &[ParseWarning]) -> PyResult<()> {
    if strict && !warnings.is_empty() {
        return Err(strict_rejection(warnings));
    }
    Ok(())
}

/// Put one batch slot's outcome in the list, or fail the batch.
///
/// One notion of "this slot failed", two ways to reach it: a parse error, or a
/// strict rejection. `raise_on_error=False` appends **the exception object
/// itself**, not a raise, which is the whole contract of the flag and was written
/// out three times.
pub(crate) fn push_slot(
    py: Python<'_>,
    items: &Bound<'_, PyList>,
    outcome: PyResult<Py<PyAny>>,
    raise_on_error: bool,
) -> PyResult<()> {
    match outcome {
        Ok(object) => items.append(object),
        Err(err) => {
            if raise_on_error {
                return Err(err);
            }
            items.append(err.value(py))
        }
    }
}
