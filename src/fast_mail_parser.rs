//! PyO3 binding layer for the `fast_mail_parser` extension module.
//!
//! The crate intentionally keeps two parallel data models:
//!
//! - [`mail_parser`] is a **PyO3-free core**: `Mail`/`Attachment` are plain Rust
//!   types that hold the parsed message. Because they have no Python dependency,
//!   the parsing logic can be exercised and tested independently of any Python
//!   runtime.
//! - Everything under this root is the **PyO3 binding layer**: `PyMail` and
//!   friends wrap the core types and expose them to Python, converting Rust
//!   values into Python objects (e.g. `Vec<u8>` -> `bytes`).
//!
//! Keeping the split decouples the parsing logic from the Python bindings: the
//! core stays portable and unit-testable, while everything PyO3-specific lives
//! here.
//!
//! This file is the module list and the `#[pymodule]`, and nothing else (#233).
//! The binding was one 2000-line file, which meant a change to one mode could not
//! be reviewed without loading the other seventeen hundred lines. What each
//! module holds:
//!
//! | module | what is in it |
//! |---|---|
//! | [`errors`] | the four exception types, the mapping onto them, the panic backstop |
//! | [`payload`] | reading a caller's `bytes`/`str`, and pinning it for the deferred modes |
//! | [`convert`] | conversions every result type would otherwise repeat |
//! | [`metadata`] | `mode="metadata"`: an inventory, nothing decoded |
//! | [`flat`] | the default projection, and the warning channel |
//! | [`lazy`] | `mode="lazy"`: bodies decoded, attachments deferred |
//! | [`tree`] | the MIME tree, in all three modes |
//! | [`api`] | the three `#[pyfunction]`s and their mode dispatch |
//!
//! Every `#[inline(never)]`, `#[cold]` and `#[inline(always)]` stayed on the item
//! it was on: 21, 5 and 1 of them, the same counts as before the split. Module
//! boundaries do not change inlining under this crate's `lto = true,
//! codegen-units = 1`, but the attributes are load-bearing regardless -- see the
//! note on [`errors::catch_panics`] for what it cost when one was lost.

// The parsing core is its own crate now (#236), so it can be tested directly
// and cannot accidentally acquire a PyO3 dependency. Aliased to the old module
// name so every call site below reads unchanged.
mod api;
mod convert;
mod errors;
mod flat;
mod lazy;
mod metadata;
mod payload;
mod tree;

pub(crate) use fast_mail_parser_core as mail_parser;

use pyo3::prelude::*;
use pyo3::wrap_pyfunction;

use api::{_panic_for_tests, parse_email, parse_email_tree, parse_many};
use errors::{DecodeError, HeaderParseError, MimeStructureError, ParseError};
use flat::{ParseWarning, PyAddress, PyAttachment, PyMail};
use lazy::{PyLazyAttachment, PyLazyMail};
use metadata::{PyAttachmentMetadata, PyMailMetadata};
use tree::{PyLazyMimePart, PyMimePart, PyMimePartMetadata};
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
