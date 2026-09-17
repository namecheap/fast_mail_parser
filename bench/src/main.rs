//! Where the time actually goes in a parse.
//!
//! ```text
//! cargo run --release -- ../tests/data/large_message.eml full
//! cargo run --release -- ../tests/data/large_message.eml strip --rounds 200
//! ```
//!
//! The pytest suite is the oracle for what the wheel costs a user. This is for
//! the question it cannot answer: of that cost, which loop is it? The `strip`
//! and `b64-*` modes isolate the two halves of the base64 path -- most of a full
//! parse on the large fixture -- and time them separately, which nothing in the
//! repository could do before. `b64-simd` against `b64-scalar` is what #228
//! bought, measured on its own rather than through a whole parse.

// The core crate the extension links; see Cargo.toml.
use fast_mail_parser_core as mail_parser;

// The vendored whitespace strip, so `strip` measures the code that ships rather
// than a reimplementation of it. Same reason for the allow: the module carries
// helpers the strip uses internally and this binary names only the entry point.
#[allow(dead_code)]
#[path = "../../vendor/mailparse/src/bytescan.rs"]
mod bytescan;

use std::time::{Duration, Instant};

const USAGE: &str = "usage: bench <fixture.eml> <mode> [--rounds N]\n\
                     modes: full metadata lazy tree tree-lazy tree-metadata decode-part strip \
                     b64-simd b64-scalar";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    let (path, mode) = (&args[0], args[1].as_str());
    let rounds: usize = match args.iter().position(|a| a == "--rounds") {
        Some(i) => args
            .get(i + 1)
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| {
                eprintln!("--rounds needs a number");
                std::process::exit(2)
            }),
        None => 50,
    };

    let payload = std::fs::read(path).unwrap_or_else(|err| {
        eprintln!("{path}: {err}");
        std::process::exit(2)
    });

    // The isolated loops work on one part, not the whole message, so this is
    // prepared once and outside the timing. Two different slices are needed:
    // `decode_part` re-parses a part, so it wants the part *including its
    // headers*, while the strip and the base64 decode want only the encoded
    // body. Handing the body to `decode_part` makes it parse a megabyte as a
    // header block -- which it does, slowly, and then fails.
    let part = largest_leaf(&payload);
    let raw_part = part.as_ref().map(|(raw, _)| raw.clone());
    let encoded = part.as_ref().map(|(_, body)| body.clone());
    let stripped = encoded
        .as_ref()
        .map(|raw| bytescan::strip_ascii_whitespace(raw));

    let run: Box<dyn Fn() -> usize> = match mode {
        "full" => Box::new(|| {
            mail_parser::parse_email(&payload)
                .expect("the fixture must parse")
                .headers
                .len()
        }),
        "metadata" => Box::new(|| {
            mail_parser::parse_email_metadata(&payload)
                .expect("the fixture must parse")
                .headers
                .len()
        }),
        "lazy" => Box::new(|| {
            mail_parser::parse_email_lazy(&payload)
                .expect("the fixture must parse")
                .attachments
                .len()
        }),
        "tree" => Box::new(|| {
            mail_parser::parse_email_tree(&payload)
                .expect("the fixture must parse")
                .children
                .len()
        }),
        "tree-lazy" => Box::new(|| {
            mail_parser::parse_tree_deferred(&payload, true)
                .expect("the fixture must parse")
                .children
                .len()
        }),
        "tree-metadata" => Box::new(|| {
            mail_parser::parse_tree_deferred(&payload, false)
                .expect("the fixture must parse")
                .children
                .len()
        }),
        "decode-part" => {
            let raw = require(raw_part, "decode-part needs a fixture with an attachment");
            Box::new(move || {
                mail_parser::decode_part(&raw)
                    .expect("decode_part must succeed; a failing mode would time the error path")
                    .len()
            })
        }
        "strip" => {
            let raw = require(encoded.clone(), "strip needs a fixture with an attachment");
            Box::new(move || bytescan::strip_ascii_whitespace(&raw).len())
        }
        // The decoder that ships, since #228.
        "b64-simd" => {
            let clean = require(
                stripped.clone(),
                "b64-simd needs a fixture with an attachment",
            );
            Box::new(move || {
                base64_simd::STANDARD
                    .decode_to_vec(&clean)
                    .expect("the stripped body must decode; otherwise this times a failure")
                    .len()
            })
        }
        // The decoder it replaced, kept so the trade #228 made is measurable in
        // isolation rather than only through a whole parse.
        "b64-scalar" => {
            let clean = require(
                stripped.clone(),
                "b64-scalar needs a fixture with an attachment",
            );
            Box::new(move || {
                data_encoding::BASE64_MIME_PERMISSIVE
                    .decode(&clean)
                    .expect("the stripped body must decode; otherwise this times a failure")
                    .len()
            })
        }
        other => {
            eprintln!("unknown mode {other:?}\n{USAGE}");
            std::process::exit(2)
        }
    };

    // Warm up: the first iterations pay page faults and branch-predictor
    // training that no later one does, and including them moves the minimum.
    for _ in 0..(rounds / 10).max(3) {
        std::hint::black_box(run());
    }

    let mut timings: Vec<Duration> = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let start = Instant::now();
        std::hint::black_box(run());
        timings.push(start.elapsed());
    }
    timings.sort_unstable();

    let us = |d: Duration| d.as_secs_f64() * 1e6;
    println!(
        "{mode:<14} rounds={rounds:<5} min={:>10.3} us  median={:>10.3} us  p90={:>10.3} us",
        us(timings[0]),
        us(timings[timings.len() / 2]),
        us(timings[timings.len() * 9 / 10]),
    );
}

fn require<T>(value: Option<T>, message: &str) -> T {
    value.unwrap_or_else(|| {
        eprintln!("{message}");
        std::process::exit(2)
    })
}

/// The largest leaf part: `(the whole part with its headers, its encoded body)`.
///
/// The largest is the interesting one -- it is where the decode time is. Both
/// slices are returned because the modes need different ones, and getting that
/// backwards is not visible in the output: it just measures something else.
/// Extraction itself is never timed.
fn largest_leaf(payload: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let parsed = mailparse::parse_mail(payload).ok()?;
    fn walk(part: &mailparse::ParsedMail<'_>, best: &mut Option<(Vec<u8>, Vec<u8>)>) {
        if part.subparts.is_empty() {
            let raw = part.get_body_encoded();
            let bytes = match raw {
                mailparse::body::Body::Base64(b) | mailparse::body::Body::QuotedPrintable(b) => {
                    b.get_raw().to_vec()
                }
                mailparse::body::Body::SevenBit(b) | mailparse::body::Body::EightBit(b) => {
                    b.get_raw().to_vec()
                }
                mailparse::body::Body::Binary(b) => b.get_raw().to_vec(),
            };
            if best
                .as_ref()
                .is_none_or(|(_, body)| body.len() < bytes.len())
            {
                *best = Some((part.raw_bytes.to_vec(), bytes));
            }
        }
        for child in &part.subparts {
            walk(child, best);
        }
    }
    let mut best = None;
    walk(&parsed, &mut best);
    best
}
