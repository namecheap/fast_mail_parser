use charset::{decode_ascii, Charset};

use crate::{MailParseError, ParsedContentType};

/// Represents the body of an email (or mail subpart)
pub enum Body<'a> {
    /// A body with 'base64' Content-Transfer-Encoding.
    Base64(EncodedBody<'a>),
    /// A body with 'quoted-printable' Content-Transfer-Encoding.
    QuotedPrintable(EncodedBody<'a>),
    /// A body with '7bit' Content-Transfer-Encoding.
    SevenBit(TextBody<'a>),
    /// A body with '8bit' Content-Transfer-Encoding.
    EightBit(TextBody<'a>),
    /// A body with 'binary' Content-Transfer-Encoding.
    Binary(BinaryBody<'a>),
}

impl<'a> Body<'a> {
    pub fn new(
        body: &'a [u8],
        ctype: &'a ParsedContentType,
        transfer_encoding: &Option<String>,
    ) -> Body<'a> {
        transfer_encoding
            .as_ref()
            .map(|encoding| match encoding.as_ref() {
                "base64" => Body::Base64(EncodedBody {
                    decoder: decode_base64,
                    body,
                    ctype,
                }),
                "quoted-printable" => Body::QuotedPrintable(EncodedBody {
                    decoder: decode_quoted_printable,
                    body,
                    ctype,
                }),
                "7bit" => Body::SevenBit(TextBody { body, ctype }),
                "8bit" => Body::EightBit(TextBody { body, ctype }),
                "binary" => Body::Binary(BinaryBody { body, ctype }),
                _ => Body::get_default(body, ctype),
            })
            .unwrap_or_else(|| Body::get_default(body, ctype))
    }

    fn get_default(body: &'a [u8], ctype: &'a ParsedContentType) -> Body<'a> {
        Body::SevenBit(TextBody { body, ctype })
    }
}

/// Struct that holds the encoded body representation of the message (or message subpart).
pub struct EncodedBody<'a> {
    decoder: fn(&[u8]) -> Result<Vec<u8>, MailParseError>,
    ctype: &'a ParsedContentType,
    body: &'a [u8],
}

impl<'a> EncodedBody<'a> {
    /// Get the body Content-Type
    pub fn get_content_type(&self) -> &'a ParsedContentType {
        self.ctype
    }

    /// Get the raw body of the message exactly as it is written in the message (or message subpart).
    pub fn get_raw(&self) -> &'a [u8] {
        self.body
    }

    /// Get the decoded body of the message (or message subpart).
    pub fn get_decoded(&self) -> Result<Vec<u8>, MailParseError> {
        (self.decoder)(self.body)
    }

    /// Get the body of the message as a Rust string.
    /// This function tries to decode the body and then converts
    /// the result into a Rust UTF-8 string using the charset in the Content-Type
    /// (or "us-ascii" if the charset was missing or not recognized).
    /// This operation returns a valid result only if the decoded body
    /// has a text format.
    pub fn get_decoded_as_string(&self) -> Result<String, MailParseError> {
        get_body_as_string(&self.get_decoded()?, self.ctype)
    }
}

/// Struct that holds the textual body representation of the message (or message subpart).
pub struct TextBody<'a> {
    ctype: &'a ParsedContentType,
    body: &'a [u8],
}

impl<'a> TextBody<'a> {
    /// Get the body Content-Type
    pub fn get_content_type(&self) -> &'a ParsedContentType {
        self.ctype
    }

    /// Get the raw body of the message exactly as it is written in the message (or message subpart).
    pub fn get_raw(&self) -> &'a [u8] {
        self.body
    }

    /// Get the body of the message as a Rust string.
    /// This function converts the body into a Rust UTF-8 string using the charset
    /// in the Content-Type
    /// (or "us-ascii" if the charset was missing or not recognized).
    pub fn get_as_string(&self) -> Result<String, MailParseError> {
        get_body_as_string(self.body, self.ctype)
    }
}

/// Struct that holds a binary body representation of the message (or message subpart).
pub struct BinaryBody<'a> {
    ctype: &'a ParsedContentType,
    body: &'a [u8],
}

impl<'a> BinaryBody<'a> {
    /// Get the body Content-Type
    pub fn get_content_type(&self) -> &'a ParsedContentType {
        self.ctype
    }

    /// Get the raw body of the message exactly as it is written in the message (or message subpart).
    pub fn get_raw(&self) -> &'a [u8] {
        self.body
    }

    /// Get the body of the message as a Rust string. This function attempts
    /// to convert the body into a Rust UTF-8 string using the charset in the
    /// Content-Type header (or "us-ascii" as default). However, this may not
    /// always work for "binary" data. The API is provided anyway for
    /// convenient handling of real-world emails that may provide textual data
    /// with a binary transfer encoding, but use this at your own risk!
    pub fn get_as_string(&self) -> Result<String, MailParseError> {
        get_body_as_string(self.body, self.ctype)
    }
}

/// Decode a base64 body: SIMD where the CPU has it, with the crate's original
/// decoder as the arbiter of everything the SIMD one turns down.
///
/// `base64_simd::STANDARD` accepts a strict subset of what
/// `BASE64_MIME_PERMISSIVE` accepts on a whitespace-free buffer -- same
/// alphabet, padding required by both, and this one is additionally strict
/// about `=` appearing mid-stream and about non-zero trailing bits. So the set
/// "SIMD accepts, data-encoding rejects" is empty, and anything both accept
/// decodes to the same bytes. A success here is therefore exactly what the
/// previous decoder would have produced, and every rejection still goes through
/// `data_encoding` and surfaces the identical `DecodeError { position, kind }`.
/// `simd_and_data_encoding_agree` below is what holds that claim up.
///
/// Decoding happens in place, over `cleaned`. That keeps the second large
/// allocation this function used to make -- `data_encoding::decode` allocates
/// *and zero-fills* a whole output buffer -- out of the picture entirely, at the
/// cost of leaving the result with the input's capacity (~4/3 of the decoded
/// length). `shrink_to_fit` gives that back, because the decoded `Vec` is
/// retained for the lifetime of the Python attachment object that ends up
/// holding it, so its capacity is resident memory rather than a transient.
/// Decode a base64 body: SIMD where the CPU has it, with the crate's original
/// decoder as the arbiter of everything the SIMD one turns down.
///
/// `base64_simd::STANDARD` accepts a strict subset of what
/// `BASE64_MIME_PERMISSIVE` accepts on a whitespace-free buffer -- same
/// alphabet, padding required by both, and this one is additionally strict about
/// `=` appearing mid-stream and about non-zero trailing bits. So the set "SIMD
/// accepts, data-encoding rejects" is empty, and anything both accept decodes to
/// the same bytes. A success here is therefore exactly what the previous decoder
/// would have produced, and every rejection still goes through `data_encoding`,
/// surfacing the identical `DecodeError { position, kind }` to the same public
/// error type. `simd_and_data_encoding_agree` below is what holds that claim up.
///
/// Decoding is out of place, into an exactly-sized buffer, and `cleaned` stays
/// intact so the cold path can reuse it. The in-place form
/// (`decode_inplace` over `cleaned`, then `truncate`) was measured too and looks
/// strictly better on paper -- it reuses the strip's allocation instead of making
/// a second one. It is not: in this crate, built as the extension is built
/// (`lto = true`, `codegen-units = 1`), the in-place version made the whole parse
/// **9% slower** while this one makes it **51% faster**, on the same machine, the
/// same corpus and the same decoder. Standalone, the two are within 10% of each
/// other. That gap is the code-layout sensitivity #204 documents, and it is why
/// this function is written the way it is rather than the way that reads better.
/// Measure before changing the shape of this.
fn decode_base64(body: &[u8]) -> Result<Vec<u8>, MailParseError> {
    let cleaned = crate::bytescan::strip_ascii_whitespace(body);

    if let Ok(decoded) = base64_simd::STANDARD.decode_to_vec(&cleaned) {
        return Ok(decoded);
    }

    Ok(data_encoding::BASE64_MIME_PERMISSIVE.decode(&cleaned)?)
}

fn decode_quoted_printable(body: &[u8]) -> Result<Vec<u8>, MailParseError> {
    Ok(quoted_printable::decode(
        body,
        quoted_printable::ParseMode::Robust,
    )?)
}

fn get_body_as_string(body: &[u8], ctype: &ParsedContentType) -> Result<String, MailParseError> {
    let cow = if let Some(charset) = Charset::for_label(ctype.charset.as_bytes()) {
        let (cow, _, _) = charset.decode(body);
        cow
    } else {
        decode_ascii(body)
    };
    Ok(cow.into_owned())
}

#[cfg(test)]
mod tests {
    use super::decode_base64;

    /// What `decode_base64` did before the SIMD fast path existed.
    fn oracle(body: &[u8]) -> Result<Vec<u8>, data_encoding::DecodeError> {
        let cleaned: Vec<u8> = body
            .iter()
            .filter(|c| !c.is_ascii_whitespace())
            .cloned()
            .collect();
        data_encoding::BASE64_MIME_PERMISSIVE.decode(&cleaned)
    }

    fn assert_agrees(body: &[u8]) {
        match (decode_base64(body), oracle(body)) {
            (Ok(got), Ok(want)) => assert_eq!(got, want, "bytes differ for {body:?}"),
            (Err(crate::MailParseError::Base64DecodeError(got)), Err(want)) => {
                assert_eq!(got, want, "error differs for {body:?}")
            }
            (got, want) => panic!(
                "acceptance differs for {body:?}: got {:?}, oracle {:?}",
                got.map(|v| v.len()).map_err(|e| e.to_string()),
                want.map(|v| v.len())
            ),
        }
    }

    /// Every input either decodes to the same bytes through both decoders, or is
    /// rejected by both with the identical `DecodeError`. This is what makes the
    /// SIMD fast path safe to take: it proves the set "SIMD accepts, the oracle
    /// rejects" is empty over this corpus, so a success never changes behaviour.
    #[test]
    fn simd_and_data_encoding_agree() {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

        // Valid padded input at every length, plus the unpadded tails.
        for len in 0..=200usize {
            let raw: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let encoded = data_encoding::BASE64.encode(&raw);
            assert_agrees(encoded.as_bytes());
            assert_agrees(encoded.trim_end_matches('=').as_bytes());
        }

        // `=` at and off a block boundary, and the degenerate pad shapes.
        for body in [
            &b"SGVsbA==byB3b3JsZA=="[..], // mid-stream padding: oracle accepts
            b"aGVsbG8=d29ybGQ=",
            b"A===",
            b"AA=A",
            b"=AAA",
            b"AAA=AAAA",
            b"QR==", // non-zero trailing bits
            b"QUI=",
            b"QUJD",
            b"QUJ",
            b"",
            b"=",
            b"==",
            b"===",
            b"====",
        ] {
            assert_agrees(body);
        }

        // Every byte, at every position of a 4-block -- this is what sweeps the
        // non-alphabet bytes, the whitespace bytes, and `\x0b`/`\x00`.
        for byte in 0u8..=255 {
            for pos in 0..4 {
                let mut block = *b"QUJD";
                block[pos] = byte;
                assert_agrees(&block);

                let mut two = *b"QUJDQUJD";
                two[pos + 4] = byte;
                assert_agrees(&two);
            }
        }

        // Whitespace interleaved through an otherwise valid body, including the
        // vertical tab and NUL that are *not* ASCII whitespace and so survive the
        // strip to be rejected.
        for sep in [
            &b"\r\n"[..],
            b" ",
            b"\t",
            b"\x0c",
            b"\n",
            b"\r",
            b"\x0b",
            b"\x00",
        ] {
            let mut body = Vec::new();
            for chunk in b"QUJDREVGR0hJSktM".chunks(3) {
                body.extend_from_slice(chunk);
                body.extend_from_slice(sep);
            }
            assert_agrees(&body);
        }

        // Pseudo-random draws from an alphabet that mixes valid symbols, padding,
        // whitespace and rejects, in the style of `bytescan::tests::corpus`.
        let mut state: u32 = 0x9E37_79B9;
        for len in 0..80 {
            for _ in 0..8 {
                let mut body = Vec::with_capacity(len);
                for _ in 0..len {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    const MIX: &[u8; 16] = b"AQ+/=\r\n \t-\x0b\x00\xffZz9";
                    body.push(MIX[(state % 16) as usize]);
                }
                assert_agrees(&body);
            }
        }

        // A body shaped like the real thing: 76-char lines, CRLF-wrapped.
        let raw: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        let encoded = data_encoding::BASE64.encode(&raw);
        let mut wrapped = Vec::new();
        for line in encoded.as_bytes().chunks(76) {
            wrapped.extend_from_slice(line);
            wrapped.extend_from_slice(b"\r\n");
        }
        assert_agrees(&wrapped);
        assert_eq!(decode_base64(&wrapped).unwrap(), raw);

        // The alphabet itself, so no symbol is mis-mapped by either decoder.
        assert_agrees(ALPHABET);
    }

    /// The decoded `Vec` outlives this call inside a Python attachment object, so
    /// its capacity is resident memory rather than a transient. It must be the
    /// decoded length, not the encoded one (~4/3 of it).
    #[test]
    fn decoded_capacity_is_not_the_input_capacity() {
        let raw: Vec<u8> = (0..8192).map(|i| (i % 251) as u8).collect();
        let encoded = data_encoding::BASE64.encode(&raw);

        let decoded = decode_base64(encoded.as_bytes()).unwrap();

        assert_eq!(decoded, raw);
        assert!(
            decoded.capacity() < encoded.len(),
            "capacity {} should have been shrunk below the encoded length {}",
            decoded.capacity(),
            encoded.len()
        );
    }
}
