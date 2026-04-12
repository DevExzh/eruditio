//! Hex decoding with SIMD-accelerated batch processing.
//!
//! Provides [`decode_hex_pairs`], which converts pairs of ASCII hex characters
//! into bytes, skipping whitespace.  The dispatch function selects the best
//! available implementation at runtime:
//!
//! | Architecture  | Backend                          |
//! |--------------|----------------------------------|
//! | x86_64       | SSSE3 (arithmetic + maddubs)     |
//! | x86 (32-bit) | SSSE3 / SSE2 (runtime detect)    |
//! | aarch64      | scalar                           |
//! | wasm32       | scalar                           |
//! | *other*      | scalar loop                      |

// ---------------------------------------------------------------------------
// Hex lookup table (used by scalar fallback)
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
// x86 / x86_64 SSSE3 implementation
// ---------------------------------------------------------------------------

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86 {
    #[cfg(target_arch = "x86")]
    use core::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use core::arch::x86_64::*;

    /// Decode 32 hex ASCII bytes into 16 output bytes using SSSE3 arithmetic.
    ///
    /// Returns `None` if any of the 32 input bytes is not a valid hex digit.
    ///
    /// Algorithm:
    /// 1. Classify each byte as digit ('0'-'9') or alpha ('A'-'F'/'a'-'f')
    ///    using saturating unsigned subtraction for range checks.
    /// 2. Compute nibble values: digits → byte − 0x30, alpha → (byte|0x20) − 0x57.
    /// 3. Pair adjacent nibbles via `_mm_maddubs_epi16(nibbles, [16, 1, …])`.
    /// 4. Pack 16-bit results into 8-bit with `_mm_packus_epi16`.
    #[target_feature(enable = "ssse3")]
    unsafe fn decode_32_hex_ssse3(src: *const u8) -> Option<[u8; 16]> {
        unsafe {
            let zero = _mm_setzero_si128();
            let ascii_0 = _mm_set1_epi8(0x30_u8 as i8);
            let nine = _mm_set1_epi8(9);
            let mask_20 = _mm_set1_epi8(0x20_u8 as i8);
            let ascii_a = _mm_set1_epi8(0x61_u8 as i8);
            let five = _mm_set1_epi8(5);
            let ten = _mm_set1_epi8(10);
            // [16, 1, 16, 1, ...] — multiplier for maddubs nibble pairing.
            let mul = _mm_set1_epi16(0x0110);

            // Load two 16-byte chunks (32 hex chars).
            let chunk0 = _mm_loadu_si128(src as *const __m128i);
            let chunk1 = _mm_loadu_si128(src.add(16) as *const __m128i);

            // --- Convert chunk to nibble values ---
            // Inline helper applied to each chunk.
            macro_rules! hex_nibbles {
                ($chunk:expr) => {{
                    // Digit: value = byte − '0', valid when (unsigned) result ≤ 9.
                    let digit_off = _mm_sub_epi8($chunk, ascii_0);
                    let is_digit = _mm_cmpeq_epi8(_mm_subs_epu8(digit_off, nine), zero);

                    // Alpha: force lowercase with |0x20, value = byte − 0x57
                    //        (equivalent to (byte|0x20) − 'a' + 10).
                    let lower = _mm_or_si128($chunk, mask_20);
                    let alpha_off = _mm_sub_epi8(lower, ascii_a);
                    let is_alpha = _mm_cmpeq_epi8(_mm_subs_epu8(alpha_off, five), zero);
                    let alpha_val = _mm_add_epi8(alpha_off, ten);

                    // Select value and validity.
                    let value = _mm_or_si128(
                        _mm_and_si128(digit_off, is_digit),
                        _mm_and_si128(alpha_val, is_alpha),
                    );
                    let valid = _mm_or_si128(is_digit, is_alpha);
                    (value, valid)
                }};
            }

            let (nibs0, valid0) = hex_nibbles!(chunk0);
            let (nibs1, valid1) = hex_nibbles!(chunk1);

            // Validate: every byte must be a hex digit.
            let all_valid = _mm_and_si128(valid0, valid1);
            if _mm_movemask_epi8(all_valid) != 0xFFFF {
                return None;
            }

            // Pair nibbles: hi_nibble * 16 + lo_nibble via maddubs.
            let paired0 = _mm_maddubs_epi16(nibs0, mul);
            let paired1 = _mm_maddubs_epi16(nibs1, mul);

            // Pack 16-bit → 8-bit (values are 0-255, no saturation needed).
            let packed = _mm_packus_epi16(paired0, paired1);

            let mut out = [0u8; 16];
            _mm_storeu_si128(out.as_mut_ptr() as *mut __m128i, packed);
            Some(out)
        }
    }

    /// SSSE3-accelerated hex pair decoding.
    ///
    /// Processes 32 hex chars (16 output bytes) at a time using arithmetic
    /// range checks and `maddubs` nibble pairing.  Falls back to scalar for
    /// any chunk containing non-hex bytes or for trailing bytes.
    #[target_feature(enable = "ssse3")]
    pub(super) unsafe fn decode_hex_pairs_ssse3(hex: &str) -> Vec<u8> {
        let bytes = hex.as_bytes();
        let len = bytes.len();
        let mut out = Vec::with_capacity(len / 2);
        let mut i = 0;

        // Process 32-byte (16-output-byte) chunks.
        while i + 32 <= len {
            // SAFETY: `i + 32 <= len`, pointer is within bounds.
            let result = unsafe { decode_32_hex_ssse3(bytes.as_ptr().add(i)) };
            match result {
                Some(decoded) => {
                    out.extend_from_slice(&decoded);
                    i += 32;
                },
                None => {
                    // Non-hex byte (or whitespace) encountered — fall back to scalar.
                    break;
                },
            }
        }

        // Scalar tail for remaining bytes (including whitespace handling).
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
/// On x86/x86_64 the SSSE3 fast-path processes 32 hex chars at a time using
/// arithmetic range checks and `maddubs` nibble pairing.  All other
/// architectures use the scalar implementation.
#[allow(unreachable_code)]
#[inline]
pub(crate) fn decode_hex_pairs(hex: &str) -> Vec<u8> {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        #[cfg(target_arch = "x86_64")]
        {
            // SSSE3 is available on virtually all x86_64 CPUs (since Core 2, 2006).
            if is_x86_feature_detected!("ssse3") {
                // SAFETY: SSSE3 is confirmed present by the runtime check.
                return unsafe { x86::decode_hex_pairs_ssse3(hex) };
            }
            // Extremely rare: x86_64 without SSSE3 → scalar fallback.
            return decode_hex_pairs_scalar(hex);
        }
        #[cfg(target_arch = "x86")]
        if is_x86_feature_detected!("ssse3") {
            // SAFETY: SSSE3 is confirmed present by the runtime check.
            return unsafe { x86::decode_hex_pairs_ssse3(hex) };
        }
    }

    // aarch64 / wasm32 / other: scalar fallback.
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
    fn exactly_32_hex_chars() {
        // Exactly one SIMD chunk.
        let hex = "000102030405060708090a0b0c0d0e0f";
        let expected: Vec<u8> = (0x00..=0x0F).collect();
        assert_eq!(decode_hex_pairs(hex), expected);
    }

    #[test]
    fn all_hex_digits() {
        // Every possible hex digit in both cases.
        let hex = "0123456789abcdefABCDEF00";
        let expected = vec![
            0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0xAB, 0xCD, 0xEF, 0x00,
        ];
        assert_eq!(decode_hex_pairs(hex), expected);
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
