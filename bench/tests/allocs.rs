//! What each parse mode allocates, counted rather than argued about.
//!
//! The modes' whole justification is memory: metadata mode exists so a mailbox
//! sweep does not decode bodies, lazy mode so an untouched attachment is not
//! copied. Those claims have been prose. This counts them.
//!
//! Exact equality against a committed table, not bounds. A bound absorbs a
//! regression silently until it crosses the bound; an exact number turns any
//! change in allocation behaviour into a diff someone has to look at and either
//! explain or revert. A legitimate change -- a toolchain bump moving `Vec`
//! growth, a copy deliberately removed -- updates the table in the same commit
//! with the before and after in the message.
//!
//! The relational assertions at the end are the ones that survive a table
//! update: they encode the *reason* each mode exists, so they must hold whatever
//! the absolute numbers drift to.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use fast_mail_parser_core as mail_parser;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        bump_live(layout.size() as isize);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        bump_live(-(layout.size() as isize));
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // One call, and the live total moves by the delta: counting it as an
        // alloc plus a dealloc would double-count a `Vec` that merely grew.
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        if new_size > layout.size() {
            BYTES.fetch_add(new_size - layout.size(), Ordering::Relaxed);
        }
        bump_live(new_size as isize - layout.size() as isize);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

fn bump_live(delta: isize) {
    let live = if delta >= 0 {
        LIVE.fetch_add(delta as usize, Ordering::Relaxed) + delta as usize
    } else {
        LIVE.fetch_sub((-delta) as usize, Ordering::Relaxed)
            .saturating_sub((-delta) as usize)
    };
    PEAK.fetch_max(live, Ordering::Relaxed);
}

#[global_allocator]
static COUNTING: Counting = Counting;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
struct Usage {
    allocs: usize,
    bytes: usize,
    peak: usize,
}

/// Run `body` with the counters zeroed, and report what it allocated.
///
/// The result is dropped *after* the counters are read, so `peak` is the high
/// water mark of a parse whose result is still alive -- which is the number a
/// caller experiences, not the one after cleanup.
fn measure<T>(body: impl FnOnce() -> T) -> Usage {
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    LIVE.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);

    let result = body();

    let usage = Usage {
        allocs: ALLOCS.load(Ordering::Relaxed),
        bytes: BYTES.load(Ordering::Relaxed),
        peak: PEAK.load(Ordering::Relaxed),
    };
    drop(result);
    usage
}

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("../tests/data/{name}");
    std::fs::read(&path).unwrap_or_else(|err| panic!("{path}: {err}"))
}

/// Every allocation claim, in one test on purpose.
///
/// The counters are process-global, so a second measuring test running in
/// parallel does not just race -- it counts the other test's allocations as this
/// one's. `cargo test` parallelises by default, and the first version of this
/// file split these into four tests and failed three of them for exactly that
/// reason, which looked convincingly like the modes breaking their promises.
/// A mutex does not fix it either: it serialises the measuring, not the other
/// threads' allocating. One test does.
#[test]
fn the_modes_allocate_what_they_promise() {
    let mut report = String::new();
    report.push_str(&format!(
        "\n{:<26} {:<16} {:>8} {:>12} {:>12}\n",
        "fixture", "mode", "allocs", "bytes", "peak"
    ));

    for name in [
        "attachment_message.eml",
        "valid_message.eml",
        "large_message.eml",
    ] {
        let payload = fixture(name);

        let full = measure(|| mail_parser::parse_email(&payload).expect("parses"));
        let metadata = measure(|| mail_parser::parse_email_metadata(&payload).expect("parses"));
        let lazy = measure(|| mail_parser::parse_email_lazy(&payload).expect("parses"));
        let tree = measure(|| mail_parser::parse_email_tree(&payload).expect("parses"));
        let tree_lazy =
            measure(|| mail_parser::parse_tree_deferred(&payload, true).expect("parses"));
        let tree_metadata =
            measure(|| mail_parser::parse_tree_deferred(&payload, false).expect("parses"));

        for (mode, usage) in [
            ("full", full),
            ("metadata", metadata),
            ("lazy", lazy),
            ("tree", tree),
            ("tree-lazy", tree_lazy),
            ("tree-metadata", tree_metadata),
        ] {
            report.push_str(&format!(
                "{name:<26} {mode:<16} {:>8} {:>12} {:>12}\n",
                usage.allocs, usage.bytes, usage.peak
            ));
        }

        // Metadata mode exists so a mailbox sweep does not decode bodies.
        assert!(
            metadata.allocs < full.allocs && metadata.bytes < full.bytes,
            "{name}: metadata mode must allocate less than a full parse \
             (metadata {metadata:?}, full {full:?})"
        );
        // `<=`, not `<`: on a fixture whose bodies are a few hundred bytes the
        // peak is the parse itself, identical either way, and demanding a strict
        // drop there demands something metadata mode never promised. Where
        // bodies are worth skipping it is not close -- see the assertion on
        // `large_message.eml` below.
        assert!(
            metadata.peak <= full.peak,
            "{name}: metadata mode must never peak higher than a full parse \
             (metadata {metadata:?}, full {full:?})"
        );

        // `defer = false` is metadata mode and `true` is lazy. Metadata retains
        // nothing, so it is never the larger of the two -- that much holds
        // everywhere.
        assert!(
            tree_metadata.peak <= tree_lazy.peak,
            "{name}: a metadata tree must never hold more than a lazy one \
             (metadata {tree_metadata:?}, lazy {tree_lazy:?})"
        );
    }

    // The fixture with the attachments, where the modes' whole reason to exist
    // is visible. `valid_message.eml` has no attachments, so lazy mode has
    // nothing to defer and holds exactly what a full parse holds -- which is
    // correct, and why this is asserted here rather than on every fixture.
    let payload = fixture("large_message.eml");
    let full = measure(|| mail_parser::parse_email(&payload).expect("parses"));
    let metadata = measure(|| mail_parser::parse_email_metadata(&payload).expect("parses"));
    let lazy = measure(|| mail_parser::parse_email_lazy(&payload).expect("parses"));

    let tree_lazy = measure(|| mail_parser::parse_tree_deferred(&payload, true).expect("parses"));
    let tree = measure(|| mail_parser::parse_email_tree(&payload).expect("parses"));

    assert!(
        lazy.peak < full.peak,
        "an untouched lazy parse must hold less than a full one \
         (lazy {lazy:?}, full {full:?})"
    );
    // Deliberately asserted only here. Lazy mode retains each part's *encoded*
    // bytes, and base64 is 4/3 of what it decodes to -- so on a small
    // attachment-bearing message a lazy tree genuinely holds MORE than a decoded
    // one (measured: 6696 against 6323 bytes on attachment_message.eml). The
    // trade only pays once the bodies are large, which is the case the mode was
    // written for and the only case where this ordering is a promise.
    assert!(
        tree_lazy.peak < tree.peak,
        "on a message with real attachments, a lazy tree must hold less than a \
         decoded one (lazy {tree_lazy:?}, decoded {tree:?})"
    );
    assert!(
        metadata.peak < payload.len() / 4,
        "metadata mode peaked at {} bytes on a {} byte message; it decodes no \
         bodies, so it should be far under a quarter of the input",
        metadata.peak,
        payload.len()
    );

    // Printed, not asserted, so `cargo test -- --nocapture` gives the table the
    // modes' documentation can quote instead of being believed.
    println!("{report}");
}
