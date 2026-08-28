//! Byte scans that look at a machine word at a time instead of a byte at a time.
//!
//! The whitespace strip before base64 decoding used to test every byte of the body on
//! its own. (In this copy the MIME boundary search uses `memchr` instead -- see
//! `find_from_u8` and PATCH.md; upstream is offered a word-at-a-time version of both.) On a message with large attachments they are most of the parse,
//! and a byte-at-a-time loop's speed also depends on where the linker happens to
//! place it -- the same instructions ran at half speed on x86-64 when the loop
//! straddled a 64-byte boundary.
//!
//! These versions read `usize`-sized chunks with `chunks_exact` and
//! `from_ne_bytes` (plain loads; no `unsafe`) and use two classic word tricks to
//! decide whether a chunk needs a closer look:
//!
//! - `below_mask(x, n)`: non-zero iff some byte of `x` is below `n`, exact for `n <= 128`.
//!
//! Where a mask is non-zero, its set bits say which bytes to look at, so a hit costs a
//! `trailing_zeros` rather than a rescan of the word.
//!
//! Both are exact, so a chunk is only examined byte by byte when it really contains
//! a candidate. See Anderson, "Bit Twiddling Hacks", `haszero` and `hasless`.

// `chunks_exact` with a constant size is what clippy would rewrite to `as_chunks`, which
// is only stable from Rust 1.88 -- above this crate's minimum supported version.
#![allow(clippy::chunks_exact_to_as_chunks)]

const WORD: usize = core::mem::size_of::<usize>();
const LO: usize = usize::from_ne_bytes([0x01; WORD]);
const HI: usize = usize::from_ne_bytes([0x80; WORD]);

/// Each byte of the result has its high bit set iff that byte of `x` is below `n`
/// (`n <= 128`), with the same caveat as `zero_byte_mask` above the lowest hit: never a
/// false negative, possibly a false positive, so callers re-check.
#[inline]
fn below_mask(x: usize, n: u8) -> usize {
    x.wrapping_sub(LO.wrapping_mul(n as usize)) & !x & HI
}

/// The word at `chunk`, byte 0 in the low-order bits, so that a mask bit at position
/// `p` refers to byte `p / 8` regardless of the machine's endianness.
#[inline]
fn word(chunk: &[u8]) -> usize {
    // Callers pass exactly `WORD` bytes. Spelled with `copy_from_slice` rather than
    // `try_into` so it reads the same in every edition; it compiles to one load.
    let mut bytes = [0u8; WORD];
    bytes.copy_from_slice(chunk);
    usize::from_le_bytes(bytes)
}

/// Byte index of the lowest set bit of a non-zero mask.
#[inline]
fn first_hit(mask: usize) -> usize {
    (mask.trailing_zeros() / 8) as usize
}

/// `body` without its ASCII whitespace -- exactly the bytes `u8::is_ascii_whitespace`
/// names: space, tab, line feed, form feed, carriage return.
///
/// Every whitespace byte is below `0x21`, so a word with no byte below `0x21` has no
/// whitespace and is skipped whole. For a word that has candidates, the mask says
/// where they are, and each is re-checked with `is_ascii_whitespace`, so `0x0B` and
/// the other control characters below `0x21` are kept, as before. Runs of kept bytes
/// are copied in one piece rather than pushed one at a time. A base64 body is mostly
/// 76-byte lines ending in CRLF, so the common case is two mask bits and one copy per
/// line.
pub(crate) fn strip_ascii_whitespace(body: &[u8]) -> Vec<u8> {
    let mut cleaned = Vec::with_capacity(body.len());
    let mut run_start = 0;
    let mut offset = 0;
    let mut words = body.chunks_exact(WORD);
    for chunk in &mut words {
        let mut mask = below_mask(word(chunk), 0x21);
        while mask != 0 {
            let i = offset + first_hit(mask);
            if body[i].is_ascii_whitespace() {
                cleaned.extend_from_slice(&body[run_start..i]);
                run_start = i + 1;
            }
            mask &= mask - 1;
        }
        offset += WORD;
    }
    for (i, &b) in words.remainder().iter().enumerate() {
        if b.is_ascii_whitespace() {
            cleaned.extend_from_slice(&body[run_start..offset + i]);
            run_start = offset + i + 1;
        }
    }
    cleaned.extend_from_slice(&body[run_start..]);
    cleaned
}

#[cfg(test)]
mod tests {
    use super::strip_ascii_whitespace;

    fn naive_strip(body: &[u8]) -> Vec<u8> {
        body.iter()
            .filter(|c| !c.is_ascii_whitespace())
            .cloned()
            .collect()
    }

    /// Deterministic pseudo-random bytes drawn from a small alphabet, so that needles
    /// actually occur, at every alignment relative to the word size.
    fn corpus() -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut state: u32 = 0x9E37_79B9;
        for len in 0..80 {
            for _ in 0..4 {
                let mut v = Vec::with_capacity(len);
                for _ in 0..len {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    const ALPHABET: &[u8; 12] = b"-\r\n \tab=\x0c\x0b\x00\xff";
                    v.push(ALPHABET[(state % 12) as usize]);
                }
                out.push(v);
            }
        }
        out
    }

    #[test]
    fn strip_matches_filter() {
        for h in corpus() {
            assert_eq!(strip_ascii_whitespace(&h), naive_strip(&h), "{:?}", h);
        }
        for b in 0u8..=255 {
            let single = [b];
            assert_eq!(
                strip_ascii_whitespace(&single),
                naive_strip(&single),
                "{:?}",
                b
            );
            let mixed = [
                b' ', b, b'\t', b, b'\r', b'\n', b, b, b, b, b, b, b, b, b, b, b'\x0c', b,
            ];
            assert_eq!(
                strip_ascii_whitespace(&mixed),
                naive_strip(&mixed),
                "{:?}",
                b
            );
        }
        // vertical tab is below 0x21 but is not ASCII whitespace: kept
        assert_eq!(
            strip_ascii_whitespace(b"a\x0bb\x0b\x0b\x0b\x0b\x0b\x0bc"),
            b"a\x0bb\x0b\x0b\x0b\x0b\x0b\x0bc"
        );
    }
}
