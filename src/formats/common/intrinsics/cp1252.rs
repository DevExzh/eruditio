//! CP-1252 (Windows-1252) decoding with SIMD-accelerated ASCII fast-path.
//!
//! Provides [`decode_cp1252`], which converts a CP-1252 byte slice to a Unicode
//! `String`.  ASCII bytes (0x00-0x7F) are bulk-copied via `from_utf8_unchecked`,
//! while non-ASCII bytes are resolved through [`CP1252_TABLE`].
//!
//! The dispatch function selects the best available SIMD backend at runtime:
//!
//! | Architecture  | Backend        |
//! |--------------|----------------|
//! | x86 / x86_64 | AVX2 (32 B) then SSE2 (16 B) fallback |
//! | aarch64       | NEON (16 B)    |
//! | wasm32        | SIMD128 (16 B) |
//! | *other*       | scalar loop    |

/// Windows-1252 to Unicode lookup table.  Bytes 0x00-0x7F and 0xA0-0xFF map
/// directly to the same Unicode code point.  Bytes 0x80-0x9F have special
/// mappings.
pub(crate) static CP1252_TABLE: [char; 256] = {
    let mut table = ['\0'; 256];
    let mut i = 0u16;
    while i < 256 {
        table[i as usize] = i as u8 as char;
        i += 1;
    }
    // Special mappings for 0x80-0x9F range.
    table[0x80] = '\u{20AC}'; // Euro sign
    // 0x81 is undefined in CP-1252; map to U+FFFD replacement character.
    table[0x81] = '\u{FFFD}';
    table[0x82] = '\u{201A}'; // Single low-9 quotation mark
    table[0x83] = '\u{0192}'; // Latin small letter f with hook
    table[0x84] = '\u{201E}'; // Double low-9 quotation mark
    table[0x85] = '\u{2026}'; // Horizontal ellipsis
    table[0x86] = '\u{2020}'; // Dagger
    table[0x87] = '\u{2021}'; // Double dagger
    table[0x88] = '\u{02C6}'; // Modifier letter circumflex accent
    table[0x89] = '\u{2030}'; // Per mille sign
    table[0x8A] = '\u{0160}'; // Latin capital letter S with caron
    table[0x8B] = '\u{2039}'; // Single left-pointing angle quotation
    table[0x8C] = '\u{0152}'; // Latin capital ligature OE
    // 0x8D is undefined in CP-1252; map to U+FFFD replacement character.
    table[0x8D] = '\u{FFFD}';
    table[0x8E] = '\u{017D}'; // Latin capital letter Z with caron
    // 0x8F is undefined in CP-1252; map to U+FFFD replacement character.
    table[0x8F] = '\u{FFFD}';
    // 0x90 is undefined in CP-1252; map to U+FFFD replacement character.
    table[0x90] = '\u{FFFD}';
    table[0x91] = '\u{2018}'; // Left single quotation mark
    table[0x92] = '\u{2019}'; // Right single quotation mark
    table[0x93] = '\u{201C}'; // Left double quotation mark
    table[0x94] = '\u{201D}'; // Right double quotation mark
    table[0x95] = '\u{2022}'; // Bullet
    table[0x96] = '\u{2013}'; // En dash
    table[0x97] = '\u{2014}'; // Em dash
    table[0x98] = '\u{02DC}'; // Small tilde
    table[0x99] = '\u{2122}'; // Trade mark sign
    table[0x9A] = '\u{0161}'; // Latin small letter s with caron
    table[0x9B] = '\u{203A}'; // Single right-pointing angle quotation
    table[0x9C] = '\u{0153}'; // Latin small ligature oe
    table[0x9E] = '\u{017E}'; // Latin small letter z with caron
    table[0x9F] = '\u{0178}'; // Latin capital letter Y with diaeresis
    table
};

/// Converts a single CP-1252 byte to its Unicode character.
#[inline]
pub(crate) fn cp1252_byte_to_char(byte: u8) -> char {
    CP1252_TABLE[byte as usize]
}

/// Byte-by-byte CP-1252 decode (portable reference implementation).
pub(crate) fn decode_cp1252_scalar(data: &[u8]) -> String {
    let mut result = String::with_capacity(data.len());
    for &b in data {
        result.push(CP1252_TABLE[b as usize]);
    }
    result
}

// ---------------------------------------------------------------------------
// x86 / x86_64  SIMD implementations
// ---------------------------------------------------------------------------

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86 {
    #[cfg(target_arch = "x86")]
    use core::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use core::arch::x86_64::*;

    use super::CP1252_TABLE;

    /// AVX2 implementation -- processes 32 bytes at a time looking for all-ASCII
    /// runs, falls back to SSE2 then scalar for non-ASCII bytes.
    #[target_feature(enable = "avx2")]
    pub(crate) unsafe fn decode_cp1252_avx2(data: &[u8]) -> String {
        let len = data.len();
        let mut result = String::with_capacity(len);
        let mut i: usize = 0;

        // --- 32-byte AVX2 chunks ---
        while i + 32 <= len {
            // SAFETY: `i + 32 <= len <= data.len()`, so the 32-byte unaligned
            // load is within bounds. AVX2 is enabled by `target_feature`.
            unsafe {
                let chunk = _mm256_loadu_si256(data.as_ptr().add(i) as *const __m256i);
                let high_bits = _mm256_movemask_epi8(chunk) as u32;
                if high_bits == 0 {
                    // All 32 bytes are ASCII (high bit clear) -- bulk copy.
                    // SAFETY: All bytes in range are 0x00-0x7F, which is valid
                    // UTF-8.
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i..i + 32]);
                    result.push_str(ascii_slice);
                    i += 32;
                    continue;
                }
            }
            // Non-ASCII byte found in this 32-byte chunk -- fall through to
            // SSE2 granularity.
            break;
        }

        // --- 16-byte SSE2 tail ---
        while i + 16 <= len {
            // SAFETY: `i + 16 <= len <= data.len()`. SSE2 is implied by AVX2.
            unsafe {
                let chunk = _mm_loadu_si128(data.as_ptr().add(i) as *const __m128i);
                let high_bits = _mm_movemask_epi8(chunk) as u32;
                if high_bits == 0 {
                    // All 16 bytes are ASCII -- bulk copy.
                    // SAFETY: All bytes in range are 0x00-0x7F, valid UTF-8.
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i..i + 16]);
                    result.push_str(ascii_slice);
                    i += 16;
                    continue;
                }
                // Find the ASCII prefix length via trailing zeros of the mask.
                let ascii_prefix = high_bits.trailing_zeros() as usize;
                if ascii_prefix > 0 {
                    // SAFETY: `ascii_prefix` bytes starting at `i` are ASCII.
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i..i + ascii_prefix]);
                    result.push_str(ascii_slice);
                }
                // Push the non-ASCII byte through the lookup table.
                result.push(CP1252_TABLE[data[i + ascii_prefix] as usize]);
                i += ascii_prefix + 1;
            }
        }

        // --- scalar tail ---
        while i < len {
            let b = data[i];
            if b < 0x80 {
                result.push(b as char);
            } else {
                result.push(CP1252_TABLE[b as usize]);
            }
            i += 1;
        }
        result
    }

    /// SSE2 implementation -- processes 16 bytes at a time, then a scalar tail.
    #[target_feature(enable = "sse2")]
    pub(crate) unsafe fn decode_cp1252_sse2(data: &[u8]) -> String {
        let len = data.len();
        let mut result = String::with_capacity(len);
        let mut i: usize = 0;

        // --- 16-byte SSE2 chunks ---
        while i + 16 <= len {
            // SAFETY: `i + 16 <= len <= data.len()`. SSE2 is enabled by
            // `target_feature`.
            unsafe {
                let chunk = _mm_loadu_si128(data.as_ptr().add(i) as *const __m128i);
                let high_bits = _mm_movemask_epi8(chunk) as u32;
                if high_bits == 0 {
                    // All 16 bytes are ASCII -- bulk copy.
                    // SAFETY: All bytes in range are 0x00-0x7F, valid UTF-8.
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i..i + 16]);
                    result.push_str(ascii_slice);
                    i += 16;
                    continue;
                }
                // Find the ASCII prefix length via trailing zeros of the mask.
                let ascii_prefix = high_bits.trailing_zeros() as usize;
                if ascii_prefix > 0 {
                    // SAFETY: `ascii_prefix` bytes starting at `i` are ASCII.
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i..i + ascii_prefix]);
                    result.push_str(ascii_slice);
                }
                // Push the non-ASCII byte through the lookup table.
                result.push(CP1252_TABLE[data[i + ascii_prefix] as usize]);
                i += ascii_prefix + 1;
            }
        }

        // --- scalar tail ---
        while i < len {
            let b = data[i];
            if b < 0x80 {
                result.push(b as char);
            } else {
                result.push(CP1252_TABLE[b as usize]);
            }
            i += 1;
        }
        result
    }
}

// ---------------------------------------------------------------------------
// aarch64  NEON implementation
// ---------------------------------------------------------------------------

#[cfg(target_arch = "aarch64")]
mod aarch64 {
    use core::arch::aarch64::*;

    use super::CP1252_TABLE;

    /// NEON implementation -- processes 16 bytes at a time looking for all-ASCII
    /// runs, falls back to scalar for non-ASCII bytes.
    pub(crate) unsafe fn decode_cp1252_neon(data: &[u8]) -> String {
        let len = data.len();
        let mut result = String::with_capacity(len);
        let mut i: usize = 0;

        while i + 16 <= len {
            // SAFETY: `i + 16 <= len <= data.len()`. NEON is always available
            // on aarch64.
            unsafe {
                let chunk = vld1q_u8(data.as_ptr().add(i));
                // Reinterpret as signed; if min >= 0, all bytes are 0x00-0x7F.
                let as_signed = vreinterpretq_s8_u8(chunk);
                if vminvq_s8(as_signed) >= 0 {
                    // All 16 bytes are ASCII -- bulk copy.
                    // SAFETY: All bytes in range are 0x00-0x7F, valid UTF-8.
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i..i + 16]);
                    result.push_str(ascii_slice);
                    i += 16;
                    continue;
                }
                // Find first non-ASCII byte: shift each byte right by 7 to get
                // 1 for bytes >= 0x80, 0 otherwise.
                let high_bits = vshrq_n_u8::<7>(chunk);
                let as_u64 = vreinterpretq_u64_u8(high_bits);
                let lo = vgetq_lane_u64::<0>(as_u64);
                let ascii_prefix = if lo != 0 {
                    (lo.trailing_zeros() / 8) as usize
                } else {
                    let hi = vgetq_lane_u64::<1>(as_u64);
                    8 + (hi.trailing_zeros() / 8) as usize
                };
                if ascii_prefix > 0 {
                    // SAFETY: `ascii_prefix` bytes starting at `i` are ASCII.
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i..i + ascii_prefix]);
                    result.push_str(ascii_slice);
                }
                // Push the non-ASCII byte through the lookup table.
                result.push(CP1252_TABLE[data[i + ascii_prefix] as usize]);
                i += ascii_prefix + 1;
            }
        }

        // --- scalar tail ---
        while i < len {
            let b = data[i];
            if b < 0x80 {
                result.push(b as char);
            } else {
                result.push(CP1252_TABLE[b as usize]);
            }
            i += 1;
        }
        result
    }
}

// ---------------------------------------------------------------------------
// wasm32  SIMD128 implementation
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod wasm {
    use core::arch::wasm32::*;

    use super::CP1252_TABLE;

    /// SIMD128 implementation -- processes 16 bytes at a time, then a scalar
    /// tail.
    #[target_feature(enable = "simd128")]
    pub(crate) unsafe fn decode_cp1252_simd128(data: &[u8]) -> String {
        let len = data.len();
        let mut result = String::with_capacity(len);
        let mut i: usize = 0;

        while i + 16 <= len {
            // SAFETY: `i + 16 <= len <= data.len()`. simd128 is enabled by
            // `target_feature`.
            unsafe {
                let chunk = v128_load(data.as_ptr().add(i) as *const v128);
                let high_bits = i8x16_bitmask(chunk) as u32;
                if high_bits == 0 {
                    // All 16 bytes are ASCII -- bulk copy.
                    // SAFETY: All bytes in range are 0x00-0x7F, valid UTF-8.
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i..i + 16]);
                    result.push_str(ascii_slice);
                    i += 16;
                    continue;
                }
                // Find the ASCII prefix length via trailing zeros of the mask.
                let ascii_prefix = high_bits.trailing_zeros() as usize;
                if ascii_prefix > 0 {
                    // SAFETY: `ascii_prefix` bytes starting at `i` are ASCII.
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i..i + ascii_prefix]);
                    result.push_str(ascii_slice);
                }
                // Push the non-ASCII byte through the lookup table.
                result.push(CP1252_TABLE[data[i + ascii_prefix] as usize]);
                i += ascii_prefix + 1;
            }
        }

        // --- scalar tail ---
        while i < len {
            let b = data[i];
            if b < 0x80 {
                result.push(b as char);
            } else {
                result.push(CP1252_TABLE[b as usize]);
            }
            i += 1;
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Dispatch function
// ---------------------------------------------------------------------------

/// Decodes a CP-1252 (Windows-1252) byte slice to a Unicode `String`.
///
/// Selects the best available SIMD implementation at runtime for bulk-copying
/// ASCII runs.  Non-ASCII bytes are resolved through [`CP1252_TABLE`].
#[allow(unreachable_code)]
pub(crate) fn decode_cp1252(data: &[u8]) -> String {
    if data.is_empty() {
        return String::new();
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: AVX2 feature is confirmed present by the runtime check.
            return unsafe { x86::decode_cp1252_avx2(data) };
        }
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: SSE2 is always available on x86_64.
            return unsafe { x86::decode_cp1252_sse2(data) };
        }
        #[cfg(target_arch = "x86")]
        if is_x86_feature_detected!("sse2") {
            // SAFETY: SSE2 feature is confirmed present by the runtime check.
            return unsafe { x86::decode_cp1252_sse2(data) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64.
        return unsafe { aarch64::decode_cp1252_neon(data) };
    }
    #[cfg(target_arch = "wasm32")]
    {
        #[cfg(target_feature = "simd128")]
        {
            // SAFETY: simd128 target feature is statically enabled.
            return unsafe { wasm::decode_cp1252_simd128(data) };
        }
    }
    decode_cp1252_scalar(data)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        assert_eq!(decode_cp1252(b""), "");
        assert_eq!(decode_cp1252_scalar(b""), "");
    }

    #[test]
    fn pure_ascii() {
        let input = b"Hello, World!";
        assert_eq!(decode_cp1252(input), "Hello, World!");
        assert_eq!(decode_cp1252_scalar(input), "Hello, World!");
    }

    #[test]
    fn euro_sign() {
        // Byte 0x80 -> U+20AC (Euro sign)
        assert_eq!(cp1252_byte_to_char(0x80), '\u{20AC}');
        assert_eq!(decode_cp1252(&[0x80]), "\u{20AC}");
    }

    #[test]
    fn smart_quotes() {
        // 0x93 = left double quote (U+201C), 0x94 = right double quote (U+201D)
        assert_eq!(cp1252_byte_to_char(0x93), '\u{201C}');
        assert_eq!(cp1252_byte_to_char(0x94), '\u{201D}');
        assert_eq!(decode_cp1252(&[0x93, 0x94]), "\u{201C}\u{201D}");
    }

    #[test]
    fn mixed_ascii_and_high() {
        // b"caf\xe9" -> "cafe" with e-acute
        assert_eq!(decode_cp1252(b"caf\xe9"), "caf\u{00E9}");
    }

    #[test]
    fn all_256_bytes_match_table() {
        let input: Vec<u8> = (0..=255).collect();
        let result = decode_cp1252(&input);
        let expected: String = CP1252_TABLE.iter().collect();
        assert_eq!(result, expected);
    }

    #[test]
    fn long_ascii_bulk_copy() {
        let sentence = "The quick brown fox jumps over the lazy dog. ";
        let input: Vec<u8> = sentence.as_bytes().repeat(100);
        let expected = sentence.repeat(100);
        assert_eq!(decode_cp1252(&input), expected);
    }

    #[test]
    fn property_simd_matches_scalar() {
        // Pseudo-random testing: compare dispatch result vs. scalar for many
        // random-ish inputs.
        let mut rng: u64 = 0xDEAD_BEEF_CAFE_BABE;

        for _ in 0..2000 {
            // xorshift64
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;

            let len = (rng % 200) as usize;

            let data: Vec<u8> = (0..len)
                .map(|_| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    (rng & 0xFF) as u8
                })
                .collect();

            let expected = decode_cp1252_scalar(&data);
            let got = decode_cp1252(&data);
            assert_eq!(
                got, expected,
                "mismatch for len={len}, data={data:?}"
            );
        }
    }
}
