//! Hex decoding with SIMD-accelerated batch processing.
//!
//! Provides [`decode_hex_pairs`], which converts pairs of ASCII hex characters
//! into bytes, skipping whitespace.  The dispatch function selects the best
//! available implementation at runtime:
//!
//! | Architecture  | Backend           |
//! |--------------|-------------------|
//! | x86_64       | SSE2 (baseline)   |
//! | x86 (32-bit) | SSE2 (runtime detect) |
//! | aarch64      | scalar (NEON vtbl is 8-byte only) |
//! | wasm32       | scalar (same reason) |
//! | *other*      | scalar loop       |

// ---------------------------------------------------------------------------
// Hex lookup table
// ---------------------------------------------------------------------------

/// Lookup table: ASCII byte value -> hex nibble value (0-15), or 0xFF for
/// non-hex bytes.
static HEX_LUT: [u8; 256] = {
    let mut table = [0xFF_u8; 256];
    table[b'0' as usize] = 0;
    table[b'1' as usize] = 1;
    table[b'2' as usize] = 2;
    table[b'3' as usize] = 3;
    table[b'4' as usize] = 4;
    table[b'5' as usize] = 5;
    table[b'6' as usize] = 6;
    table[b'7' as usize] = 7;
    table[b'8' as usize] = 8;
    table[b'9' as usize] = 9;
    table[b'a' as usize] = 10;
    table[b'b' as usize] = 11;
    table[b'c' as usize] = 12;
    table[b'd' as usize] = 13;
    table[b'e' as usize] = 14;
    table[b'f' as usize] = 15;
    table[b'A' as usize] = 10;
    table[b'B' as usize] = 11;
    table[b'C' as usize] = 12;
    table[b'D' as usize] = 13;
    table[b'E' as usize] = 14;
    table[b'F' as usize] = 15;
    table
};

// ---------------------------------------------------------------------------
// Scalar fallback
// ---------------------------------------------------------------------------

/// Scalar reference implementation: decodes hex pairs, skipping whitespace.
pub(crate) fn decode_hex_pairs_scalar(hex: &str) -> Vec<u8> {
    let bytes = hex.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if i + 1 >= bytes.len() {
            break;
        }
        let hi = HEX_LUT[bytes[i] as usize];
        let lo = HEX_LUT[bytes[i + 1] as usize];
        if hi != 0xFF && lo != 0xFF {
            out.push((hi << 4) | lo);
        }
        i += 2;
    }
    out
}

// ---------------------------------------------------------------------------
// x86 / x86_64 SSE2 implementation
// ---------------------------------------------------------------------------

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86 {
    #[cfg(target_arch = "x86")]
    use core::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use core::arch::x86_64::*;

    use super::HEX_LUT;

    /// Decode 32 hex ASCII bytes (two 16-byte SIMD loads) into 16 output bytes.
    ///
    /// Returns `None` if any byte is not a valid hex digit.  Uses SSE2 loads
    /// for bulk memory transfer but performs the actual LUT lookup in scalar
    /// code (the 256-entry LUT is too large for SSSE3 `pshufb`).
    #[target_feature(enable = "sse2")]
    unsafe fn decode_32_hex_sse2(src: *const u8) -> Option<[u8; 16]> {
        let mut buf = [0u8; 32];

        // SAFETY: caller guarantees `src` points to at least 32 readable bytes.
        // SSE2 is enabled by `target_feature`.
        unsafe {
            let chunk0 = _mm_loadu_si128(src as *const __m128i);
            let chunk1 = _mm_loadu_si128(src.add(16) as *const __m128i);
            _mm_storeu_si128(buf.as_mut_ptr() as *mut __m128i, chunk0);
            _mm_storeu_si128(buf.as_mut_ptr().add(16) as *mut __m128i, chunk1);
        }

        let mut out = [0u8; 16];
        let mut j = 0;
        while j < 16 {
            let hi = HEX_LUT[buf[j * 2] as usize];
            let lo = HEX_LUT[buf[j * 2 + 1] as usize];
            if hi == 0xFF || lo == 0xFF {
                return None;
            }
            out[j] = (hi << 4) | lo;
            j += 1;
        }
        Some(out)
    }

    /// SSE2-accelerated hex pair decoding.
    ///
    /// Processes 32 hex chars (16 output bytes) at a time.  Falls back to
    /// scalar for any chunk that contains non-hex bytes or for the trailing
    /// remainder.
    #[target_feature(enable = "sse2")]
    pub(super) unsafe fn decode_hex_pairs_sse2(hex: &str) -> Vec<u8> {
        let bytes = hex.as_bytes();
        let len = bytes.len();
        let mut out = Vec::with_capacity(len / 2);
        let mut i = 0;

        // Process 32-byte (16-output-byte) chunks.
        while i + 32 <= len {
            // SAFETY: `i + 32 <= len`, so the pointer is within bounds.
            // SSE2 is enabled by `target_feature`.
            let result = unsafe { decode_32_hex_sse2(bytes.as_ptr().add(i)) };
            match result {
                Some(decoded) => {
                    out.extend_from_slice(&decoded);
                    i += 32;
                },
                None => {
                    // Non-hex byte encountered — fall back to scalar for the rest.
                    break;
                },
            }
        }

        // Scalar tail for remaining bytes.
        while i < len {
            if bytes[i].is_ascii_whitespace() {
                i += 1;
                continue;
            }
            if i + 1 >= len {
                break;
            }
            let hi = super::HEX_LUT[bytes[i] as usize];
            let lo = super::HEX_LUT[bytes[i + 1] as usize];
            if hi != 0xFF && lo != 0xFF {
                out.push((hi << 4) | lo);
            }
            i += 2;
        }

        out
    }
}

// ---------------------------------------------------------------------------
// Dispatch function
// ---------------------------------------------------------------------------

/// Decodes pairs of hex ASCII characters into bytes.
///
/// Skips whitespace between hex digits.  Non-hex characters (other than
/// whitespace) cause the pair to be skipped.
///
/// On x86_64 the SSE2 fast-path processes 32 hex chars at a time.  On 32-bit
/// x86, SSE2 is used if detected at runtime.  All other architectures use
/// the scalar implementation.
#[allow(unreachable_code)]
pub(crate) fn decode_hex_pairs(hex: &str) -> Vec<u8> {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: SSE2 is always available on x86_64.
            return unsafe { x86::decode_hex_pairs_sse2(hex) };
        }
        #[cfg(target_arch = "x86")]
        if is_x86_feature_detected!("sse2") {
            // SAFETY: SSE2 feature is confirmed present by the runtime check.
            return unsafe { x86::decode_hex_pairs_sse2(hex) };
        }
    }

    // aarch64 / wasm32 / other: scalar fallback.
    // NEON's vtbl is 8-byte only and doesn't help for a 256-entry LUT.
    // wasm32 SIMD128 has the same limitation.
    decode_hex_pairs_scalar(hex)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        assert_eq!(decode_hex_pairs(""), b"");
    }

    #[test]
    fn basic_decode() {
        assert_eq!(decode_hex_pairs("48656c6c6f"), b"Hello");
    }

    #[test]
    fn with_whitespace() {
        assert_eq!(decode_hex_pairs("48 65 6c 6c 6f"), b"Hello");
    }

    #[test]
    fn uppercase() {
        assert_eq!(decode_hex_pairs("4F4B"), b"OK");
    }

    #[test]
    fn mixed_case() {
        assert_eq!(decode_hex_pairs("4f4B"), b"OK");
    }

    #[test]
    fn long_dense_hex() {
        // 64 hex chars = 32 output bytes — exercises the SIMD path.
        let hex = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
        let expected: Vec<u8> = (0x00..=0x1F).collect();
        assert_eq!(decode_hex_pairs(hex), expected);
    }

    #[test]
    fn odd_length_drops_trailing() {
        // "48656c6c6" has 9 hex chars — the trailing '6' has no partner.
        assert_eq!(decode_hex_pairs("48656c6c6"), b"Hell");
    }

    #[test]
    fn property_simd_matches_scalar() {
        // Pseudo-random testing: compare dispatch result vs. scalar for many
        // random-ish inputs.
        let mut rng: u64 = 0xCAFE_BABE_DEAD_BEEF;
        let hex_chars = b"0123456789abcdefABCDEF";

        for _ in 0..1000 {
            // xorshift64
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;

            let len = (rng % 200) as usize;
            let input: String = (0..len)
                .map(|_| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    hex_chars[(rng % hex_chars.len() as u64) as usize] as char
                })
                .collect();

            let expected = decode_hex_pairs_scalar(&input);
            let got = decode_hex_pairs(&input);
            assert_eq!(got, expected, "mismatch for input of length {len}");
        }
    }
}
