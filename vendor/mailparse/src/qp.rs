//! Quoted-printable body decoding, a run at a time instead of a byte at a time.
//!
//! This replaces `quoted_printable::decode(body, ParseMode::Robust)` for message
//! *bodies* only; the RFC 2047 encoded-word path in `header.rs` still uses the
//! crate, where inputs are header-sized and the semantics differ.
//!
//! The crate's decoder makes three passes over the body and a fourth allocation
//! pass on top: it copies every byte into a `String` through a `filter_map`, walks
//! that with `lines()` and `trim_end()`, then decodes with one bounds-checked
//! `push` per byte into a `Vec` that was never given a capacity. On the 90 KB
//! quoted-printable HTML part in `tests/data/valid_message.eml` that is most of
//! the parse.
//!
//! This version reserves the output once, finds line breaks and escapes with
//! `memchr`, and copies everything between them with `extend_from_slice`. The
//! common case -- a line with no `=` in it -- is one copy.
//!
//! **Output is byte-identical to the crate's Robust mode.** That is not an
//! aspiration: `decode_robust_matches_the_crate` below compares the two over a
//! generated corpus at every length, over every `=xy` byte pair, and over
//! hand-written cases for each rule, and the `qp_agreement` fuzz target compares
//! them on arbitrary input. The rules being reproduced, read off the crate's
//! source rather than the RFC (the RFC is not what this library shipped):
//!
//! 1. Bytes outside `{0x09, 0x0A, 0x0D} ∪ [0x20, 0x7E]` are dropped *before*
//!    anything else, so they affect neither line splitting nor trimming.
//! 2. Lines split on LF, with a CR immediately before it belonging to the
//!    terminator (`str::lines` semantics). A final line without LF is a line.
//! 3. Each line is trimmed of trailing space, tab and CR.
//! 4. `=` + two hex digits is one byte; `=` at end of line is a soft break; `=X`
//!    at end of line is literal and the line still ends; `=XY` with a non-hex
//!    digit is three literal bytes and decoding continues after `Y`.
//! 5. CRLF joins consecutive lines, except after a soft break.
//! 6. If the filtered input ends with LF, one CRLF is appended -- even when the
//!    last line ended in a soft break.

use memchr::memchr;

/// True for the bytes the crate's `filter_map` keeps.
///
/// Written as arithmetic rather than `matches!` so that `first_dropped`'s chunk
/// reduction has no branches in it: `b - 0x20 < 0x5F` is `0x20..=0x7E` and
/// `b - 9 < 2` is TAB and LF. The set is identical -- `is_kept_is_the_same_set`
/// checks all 256 bytes against the original spelling -- but the branchy version
/// stopped LLVM vectorising the scan, and that scan is 40% of decoding a body
/// with no dropped bytes in it at all.
#[inline]
fn is_kept(byte: u8) -> bool {
    byte.wrapping_sub(0x20) < 0x5F || byte.wrapping_sub(9) < 2 || byte == b'\r'
}

/// Offset of the first byte rule 1 drops, or `None` when there is none.
///
/// A chunk at a time, because `position` has to stop at the first hit and so
/// cannot be vectorised, while a fixed-size chunk reduced with `|=` has no early
/// exit and can be. The byte-wise scan then runs only inside the one chunk that
/// failed. Nearly every real body keeps every byte, which is the case this makes
/// fast: 120 KB scans in 5.5 us against 63 us for the `position` loop.
fn first_dropped(input: &[u8]) -> Option<usize> {
    const CHUNK: usize = 32;
    let mut base = 0;
    for chunk in input.chunks_exact(CHUNK) {
        let mut dropped = 0u8;
        for &byte in chunk {
            dropped |= !is_kept(byte) as u8;
        }
        if dropped != 0 {
            return chunk
                .iter()
                .position(|&byte| !is_kept(byte))
                .map(|i| base + i);
        }
        base += CHUNK;
    }
    input[base..]
        .iter()
        .position(|&byte| !is_kept(byte))
        .map(|i| base + i)
}

/// Trailing bytes a line is trimmed of. After the filter these are the only
/// characters `str::trim_end` can see inside a line, which is why this is not
/// `trim_ascii_end` -- that set also includes the form feed the filter dropped.
#[inline]
fn is_trimmed(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r')
}

/// `input` without the bytes rule 1 drops. Returns `None` when there are none,
/// which is the common case and saves the copy.
fn filter_dropped(input: &[u8]) -> Option<Vec<u8>> {
    let first = first_dropped(input)?;

    let mut out = Vec::with_capacity(input.len());
    out.extend_from_slice(&input[..first]);
    let mut run_start = None;
    for (i, &byte) in input.iter().enumerate().skip(first + 1) {
        if is_kept(byte) {
            run_start.get_or_insert(i);
        } else if let Some(start) = run_start.take() {
            out.extend_from_slice(&input[start..i]);
        }
    }
    if let Some(start) = run_start {
        out.extend_from_slice(&input[start..]);
    }
    Some(out)
}

#[inline]
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Decode one already-trimmed line into `out`.
///
/// Returns `false` when the line ended in a soft break (a trailing `=`), which is
/// the one case that suppresses the CRLF before the next line.
fn decode_line(line: &[u8], out: &mut Vec<u8>) -> bool {
    let mut pos = 0;
    while pos < line.len() {
        // Look under the cursor first. After an escape the next byte is very
        // often another `=` -- every non-ASCII character encodes as two or three
        // consecutive escapes -- and calling `memchr` to be told the match is at
        // offset 0 pays its SIMD setup for nothing. A body that is mostly escapes
        // made that call 40,000 times and decoded 42% slower than the crate this
        // replaced; with the check it is faster than the crate on that shape and
        // 20% faster on ordinary mail too.
        let eq = if line[pos] == b'=' {
            pos
        } else {
            let Some(offset) = memchr(b'=', &line[pos..]) else {
                out.extend_from_slice(&line[pos..]);
                return true;
            };
            let eq = pos + offset;
            out.extend_from_slice(&line[pos..eq]);
            eq
        };

        match line.get(eq + 1) {
            // `=` is the last byte: a soft break. Nothing is emitted and the next
            // line joins this one directly.
            None => return false,
            Some(&upper) => match line.get(eq + 2) {
                // `=X` at the end of a line: literal, and the line ends normally.
                None => {
                    out.push(b'=');
                    out.push(upper);
                    return true;
                }
                Some(&lower) => match (hex_value(upper), hex_value(lower)) {
                    (Some(hi), Some(lo)) => {
                        out.push(hi << 4 | lo);
                        pos = eq + 3;
                    }
                    // Not a hex octet: three literal bytes, and decoding resumes
                    // after them -- so `==3D` decodes to `==3D`, not `=` + `=`.
                    _ => {
                        out.push(b'=');
                        out.push(upper);
                        out.push(lower);
                        pos = eq + 3;
                    }
                },
            },
        }
    }
    true
}

/// Decode a quoted-printable body exactly as `quoted_printable`'s Robust mode does.
pub(crate) fn decode_robust(input: &[u8]) -> Vec<u8> {
    let filtered = filter_dropped(input);
    let src: &[u8] = match filtered {
        Some(ref bytes) => bytes,
        None => input,
    };

    // Decoding never grows a line, and the only growth is the CRLF per line
    // ending, which the input already spends at least one byte on. Two spare
    // bytes cover the trailing CRLF of rule 6.
    let mut out = Vec::with_capacity(src.len() + 2);

    // `None` before the first line, mirroring the crate's `add_line_break`: the
    // CRLF is emitted *before* the next line rather than after the previous one,
    // so a soft break simply never arms it.
    let mut pending_crlf = false;
    let mut pos = 0;
    while pos < src.len() {
        let (line_end, next) = match memchr(b'\n', &src[pos..]) {
            Some(offset) => (pos + offset, pos + offset + 1),
            None => (src.len(), src.len()),
        };

        // Rule 2: a CR immediately before the LF is part of the terminator.
        let mut line = &src[pos..line_end];
        if line_end < src.len() && line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        // Rule 3.
        while let Some(&last) = line.last() {
            if is_trimmed(last) {
                line = &line[..line.len() - 1];
            } else {
                break;
            }
        }

        if pending_crlf {
            out.extend_from_slice(b"\r\n");
        }
        pending_crlf = decode_line(line, &mut out);

        pos = next;
    }

    // Rule 6: a trailing LF always contributes a CRLF, soft break or not.
    if src.last() == Some(&b'\n') {
        out.extend_from_slice(b"\r\n");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::decode_robust;

    /// The decoder this replaces. Every assertion below is against it rather than
    /// against the RFC: what must not change is what this library shipped.
    fn oracle(input: &[u8]) -> Vec<u8> {
        quoted_printable::decode(input, quoted_printable::ParseMode::Robust)
            .expect("Robust mode never fails")
    }

    #[track_caller]
    fn assert_agrees(input: &[u8]) {
        assert_eq!(
            decode_robust(input),
            oracle(input),
            "decoded differently for {input:?}"
        );
    }

    /// Same shape as `bytescan::tests::corpus`: a cheap xorshift over an alphabet
    /// chosen so every rule is reachable -- escape introducer, hex and non-hex
    /// digits, the three trimmed bytes, the line terminator, and three bytes the
    /// filter drops (`\x0b`, `\x0c`, `\x7f`, `\x00`, `\xff`).
    fn corpus() -> Vec<Vec<u8>> {
        const ALPHABET: &[u8] = b"=3DafZ \t\r\n\x0b\x0c\x7f\x00\xff";
        let mut out = Vec::new();
        let mut state: u32 = 0x9E37_79B9;
        for len in 0..=96 {
            for _ in 0..6 {
                let mut v = Vec::with_capacity(len);
                for _ in 0..len {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    v.push(ALPHABET[(state as usize) % ALPHABET.len()]);
                }
                out.push(v);
            }
        }
        out
    }

    /// `is_kept` was respelled as arithmetic to let `first_dropped` vectorise.
    /// This is the proof that it is the same set, byte for byte.
    #[test]
    fn is_kept_is_the_same_set() {
        for byte in 0u8..=255 {
            let original = matches!(byte, b'\t' | b'\r' | b'\n' | b' '..=b'~');
            assert_eq!(super::is_kept(byte), original, "byte {byte:#04x}");
        }
    }

    #[test]
    fn decode_robust_matches_the_crate() {
        for input in corpus() {
            assert_agrees(&input);
        }
    }

    /// Every possible two-byte tail after `=`, in the four positions where the
    /// crate's escape handling branches differently.
    #[test]
    fn every_escape_pair_matches_the_crate() {
        for x in 0u8..=255 {
            assert_agrees(&[b'=', x]);
            assert_agrees(&[b'=', x, b'\n']);
            assert_agrees(&[b'a', b'=', x]);
            for y in 0u8..=255 {
                assert_agrees(&[b'=', x, y]);
                assert_agrees(&[b'=', x, y, b'\n']);
            }
        }
    }

    /// One case per documented rule, named so a failure says which rule broke.
    #[test]
    fn each_rule_matches_the_crate() {
        for input in [
            &b""[..],
            b"\n",
            b"\r\n",
            b"abc",
            b"abc\n",
            // rule 4: soft break, and rule 6 on top of it
            b"abc=\n",
            b"a=\nb\n",
            b"a=\nb",
            // rule 4: `=X` at end of line is literal and the line still ends
            b"abc=A\nnext\n",
            b"abc=\r\n",
            // rule 4: non-hex octet is three literal bytes, decoding resumes
            b"==3D",
            b"=3D=3D",
            b"=ZZ",
            b"=3",
            b"=g0",
            b"=0g",
            // lowercase hex is accepted in Robust mode
            b"=3d=e2=80=99",
            // rule 3: trailing space/tab/CR are trimmed
            b"x=  \r\n",
            b"x   \n",
            b"x\t\t\n",
            // rule 2: a bare CR is not a terminator
            b"a\r\r\n",
            b"a\rb\n",
            b"a\rb",
            // rule 1: dropped bytes do not participate in trimming or splitting
            b"abc \x80\n",
            b"abc=\x80\n",
            b"a\x00b\n",
            b"\x0b\x0c\x7f",
            b"=\x803D",
            // empty lines each produce a CRLF
            b"a\n\n\nb\n",
            b"\n\n",
            // the real fixture's shape: soft-wrapped lines with escapes
            b"Lorem ipsum dolor sit amet =E2=80=94 consectetur=\r\nadipiscing elit.\r\n",
        ] {
            assert_agrees(input);
        }
    }

    /// The property the benchmark depends on: the real fixture decodes the same.
    #[test]
    fn the_valid_message_fixture_matches_the_crate() {
        let raw = include_bytes!("../../../tests/data/valid_message.eml");
        assert_agrees(raw);
    }
}

