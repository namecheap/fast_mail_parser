//! The three `#[pyfunction]`s and their mode dispatch (#233).
//!
//! Every entry point the extension exports, and nothing else: what each `mode=`
//! means, where `strict=` is answered, and how a batch's slots are filled.

use std::num::NonZeroUsize;
use std::sync::OnceLock;

use pyo3::exceptions;
use pyo3::prelude::*;
use pyo3::types::PyList;

use crate::convert::{push_slot, strict_gate};
use crate::errors::{catch_panics, to_py_err};
use crate::flat::PyMail;
use crate::lazy::PyLazyMail;
use crate::mail_parser;
use crate::metadata::PyMailMetadata;
use crate::payload::{payload_to_bytes, Payload, Pinned};
use crate::tree::{lazy_node, metadata_node, PyMimePart};

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
/// attachment's content decoded on first access and cached. A deferred
/// attachment points into `payload` rather than copying itself out of it, so the
/// result keeps `payload` alive -- see [`Pinned`]. `mode="metadata"` returns a
/// [`PyMailMetadata`] and decodes nothing at all, and pins nothing.
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
            strict_gate(true, &mail.warnings)?;
            return Ok(Py::new(py, mail)?.into_any());
        }
        let mail = parse_email_inner(py, payload)?;
        strict_gate(true, &mail.warnings)?;
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
    // retains per attachment is that part's offsets in the payload, so the
    // payload is moved into the result rather than dropped here -- see `Pinned`.
    let mail = py
        .detach(|| mail_parser::parse_email_lazy(message.as_ref()))
        .map_err(to_py_err)?;

    PyLazyMail::from_lazy(py, mail, message)
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

    PyMail::from_mail(py, mail)
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
        Some(other) => Ok(NonZeroUsize::new(other)),
        None => Ok(Some(default_workers())),
    }
}

/// The machine's parallelism, probed once per process.
///
/// `std::thread::available_parallelism` is not memoised: on Linux it reads
/// `/proc/self/cgroup`, `/proc/self/mountinfo` and the cgroup `cpu.max` files on
/// every call, which is several file opens per `parse_many` -- on the platform
/// the CI gate and production both run on, and for a value that does not move.
///
/// The cache lives here, in the binding layer, and not in `mail_parser`: that
/// module's docs rule out `static`s, `OnceLock` and `thread_local` so its
/// free-threading audit stays valid, and the core keeps its own
/// `available_parallelism` fallback for the fuzz targets that include it by path.
/// This layer already caches on Python objects the same way.
///
/// The trade-off, stated: a cgroup CPU quota changed after the first
/// `parse_many` of the process is not observed. Pass `threads=` to override.
fn default_workers() -> NonZeroUsize {
    static DEFAULT_WORKERS: OnceLock<NonZeroUsize> = OnceLock::new();

    *DEFAULT_WORKERS
        .get_or_init(|| std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN))
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
    // reference to each payload object plus its buffer pointer, not a copy of
    // its contents, so the batch is no longer duplicated in full (#96, #226).
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
                let mail = PyMail::from_mail(py, mail)?;
                match strict_gate(strict, &mail.warnings) {
                    Ok(()) => Ok(Py::new(py, mail)?.into_any()),
                    Err(err) => Err(err),
                }
            }
            Err(error) => Err(to_py_err(error)),
        };
        push_slot(py, &items, outcome, raise_on_error)?;
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

    // No strict arm: this mode never reads a body, so it has no warning list to
    // judge and `strict=True` was rejected at the boundary. The slot semantics are
    // the same ones, though, which is why it goes through the same helper.
    let items = PyList::empty(py);
    for result in parsed {
        let outcome = match result {
            Ok(metadata) => Ok(Py::new(py, PyMailMetadata::from_metadata(metadata))?.into_any()),
            Err(error) => Err(to_py_err(error)),
        };
        push_slot(py, &items, outcome, raise_on_error)?;
    }
    Ok(items.unbind())
}

/// `parse_many(..., mode="lazy")`: bodies decoded, attachments deferred (#202).
///
/// `strict=True` means here what it means everywhere else, and is honoured
/// per slot the way full mode honours it: lazy mode finds every repair the full
/// parse finds, so the verdict is the same verdict.
///
/// Worth a caution the single-message mode does not need: each slot's
/// attachments pin that slot's payload (#239), so holding the whole batch holds
/// every payload in it. One pin per slot rather than one for the batch, so
/// keeping one attachment keeps one message -- but the batch as a whole is still
/// the batch. Use it to sweep a batch and pull a few parts out of it, not to hold
/// ten thousand messages undecoded.
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

    // Zipped, not indexed: each result is paired with the payload it was parsed
    // from, and that payload is what its attachments' offsets index. The borrow
    // `parse_many_as` took ended when it returned, so the payloads can be moved
    // into the results one by one -- one pin per slot, never one for the batch.
    let items = PyList::empty(py);
    for (message, result) in messages.into_iter().zip(parsed) {
        // Folded into one `Err` arm as in full mode: one notion of "this slot
        // failed", two ways to reach it.
        let outcome = match result {
            Ok(mail) => {
                let mail = PyLazyMail::from_lazy(py, mail, message)?;
                match strict_gate(strict, &mail.warnings) {
                    Ok(()) => Ok(Py::new(py, mail)?.into_any()),
                    Err(err) => Err(err),
                }
            }
            Err(error) => Err(to_py_err(error)),
        };
        push_slot(py, &items, outcome, raise_on_error)?;
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
            let base = Pinned::new(message, tree.repaired);
            return Ok(Py::new(py, lazy_node(py, tree.root, &base)?)?.into_any());
        }
        // Metadata mode retains nothing, so the payload is dropped here as it
        // always was.
        Ok(Py::new(py, metadata_node(py, tree.root)?)?.into_any())
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
pub(crate) fn _panic_for_tests() -> PyResult<()> {
    catch_panics(panic_now)
}

fn panic_now() -> PyResult<()> {
    panic!("deliberate panic from _panic_for_tests")
}
