//! CP-1252 (Windows-1252) decoding with SIMD-accelerated ASCII fast-path.
//!
//! Provides [`decode_cp1252`], which converts a CP-1252 byte slice to a Unicode
//! `String`.  ASCII bytes (0x00-0x7F) are bulk-copied via `from_utf8_unchecked`,
//! while non-ASCII bytes are resolved through [`CP1252_TABLE`].
//!
//! The dispatch function selects the best available SIMD backend at runtime:
//!
//! | Architecture  | Backend                                        |
//! |--------------|------------------------------------------------|
//! | x86 / x86_64  | AVX512BW (64 B) then AVX2 (32 B) then SSE2 fallback |
//! | aarch64       | NEON (16 B)                            |
//! | wasm32        | SIMD128 (16 B)                         |
//! | *other*       | scalar loop                            |

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
    // 0x9D is undefined in CP-1252; map to U+FFFD replacement character.
    table[0x9D] = '\u{FFFD}';
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

    /// AVX512BW implementation -- processes 64 bytes at a time looking for
    /// all-ASCII runs, falls back to AVX2/SSE2 then scalar for non-ASCII bytes.
    ///
    /// Uses `_mm512_movepi8_mask` to extract high bits into a 64-bit mask
    /// register, doubling throughput over AVX2 for ASCII-heavy text.
    #[target_feature(enable = "avx512bw")]
    pub(super) unsafe fn decode_cp1252_avx512bw(data: &[u8]) -> String {
        let len = data.len();
        let mut result = String::with_capacity(len);
        let mut i: usize = 0;

        // --- 64-byte AVX512BW chunks ---
        while i + 64 <= len {
            unsafe {
                let chunk = _mm512_loadu_si512(data.as_ptr().add(i) as *const __m512i);
                let high_bits = _mm512_movepi8_mask(chunk);
                if high_bits == 0 {
                    // All 64 bytes are ASCII (high bit clear) -- bulk copy.
                    // SAFETY: All bytes in range are 0x00-0x7F, which is valid
                    // UTF-8.
                    let ascii_slice = core::str::from_utf8_unchecked(&data[i..i + 64]);
                    result.push_str(ascii_slice);
                    i += 64;
                    continue;
                }
                // Walk the entire 64-bit bitmask: process ALL non-ASCII bytes
                // in this chunk before advancing, avoiding redundant SIMD reloads.
                let mut mask = high_bits;
                let mut chunk_pos = 0usize;
                while mask != 0 {
                    let next_non_ascii = mask.trailing_zeros() as usize;
                    // Copy ASCII bytes between chunk_pos and next_non_ascii.
                    if next_non_ascii > chunk_pos {
                        // SAFETY: bytes in [i+chunk_pos .. i+next_non_ascii]
                        // have their high bit clear (0x00-0x7F), valid UTF-8.
                        let ascii_slice = core::str::from_utf8_unchecked(
                            &data[i + chunk_pos..i + next_non_ascii],
                        );
                        result.push_str(ascii_slice);
                    }
                    // Decode the non-ASCII byte via LUT.
                    result.push(CP1252_TABLE[data[i + next_non_ascii] as usize]);
                    chunk_pos = next_non_ascii + 1;
                    // Clear the lowest set bit.
                    mask &= mask - 1;
                }
                // Copy remaining ASCII bytes after the last non-ASCII byte.
                if chunk_pos < 64 {
                    // SAFETY: remaining bytes have high bit clear, valid UTF-8.
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i + chunk_pos..i + 64]);
                    result.push_str(ascii_slice);
                }
                i += 64;
            }
        }

        // --- 32-byte AVX2 tail ---
        while i + 32 <= len {
            unsafe {
                let chunk = _mm256_loadu_si256(data.as_ptr().add(i) as *const __m256i);
                let high_bits = _mm256_movemask_epi8(chunk) as u32;
                if high_bits == 0 {
                    let ascii_slice = core::str::from_utf8_unchecked(&data[i..i + 32]);
                    result.push_str(ascii_slice);
                    i += 32;
                    continue;
                }
                let mut mask = high_bits;
                let mut chunk_pos = 0usize;
                while mask != 0 {
                    let next_non_ascii = mask.trailing_zeros() as usize;
                    if next_non_ascii > chunk_pos {
                        let ascii_slice = core::str::from_utf8_unchecked(
                            &data[i + chunk_pos..i + next_non_ascii],
                        );
                        result.push_str(ascii_slice);
                    }
                    result.push(CP1252_TABLE[data[i + next_non_ascii] as usize]);
                    chunk_pos = next_non_ascii + 1;
                    mask &= mask - 1;
                }
                if chunk_pos < 32 {
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i + chunk_pos..i + 32]);
                    result.push_str(ascii_slice);
                }
                i += 32;
            }
        }

        // --- 16-byte SSE2 tail ---
        while i + 16 <= len {
            unsafe {
                let chunk = _mm_loadu_si128(data.as_ptr().add(i) as *const __m128i);
                let high_bits = _mm_movemask_epi8(chunk) as u32;
                if high_bits == 0 {
                    let ascii_slice = core::str::from_utf8_unchecked(&data[i..i + 16]);
                    result.push_str(ascii_slice);
                    i += 16;
                    continue;
                }
                let mut mask = high_bits;
                let mut chunk_pos = 0usize;
                while mask != 0 {
                    let next_non_ascii = mask.trailing_zeros() as usize;
                    if next_non_ascii > chunk_pos {
                        let ascii_slice = core::str::from_utf8_unchecked(
                            &data[i + chunk_pos..i + next_non_ascii],
                        );
                        result.push_str(ascii_slice);
                    }
                    result.push(CP1252_TABLE[data[i + next_non_ascii] as usize]);
                    chunk_pos = next_non_ascii + 1;
                    mask &= mask - 1;
                }
                if chunk_pos < 16 {
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i + chunk_pos..i + 16]);
                    result.push_str(ascii_slice);
                }
                i += 16;
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

    /// AVX2 implementation -- processes 32 bytes at a time looking for all-ASCII
    /// runs, falls back to SSE2 then scalar for non-ASCII bytes.
    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn decode_cp1252_avx2(data: &[u8]) -> String {
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
                    let ascii_slice = core::str::from_utf8_unchecked(&data[i..i + 32]);
                    result.push_str(ascii_slice);
                    i += 32;
                    continue;
                }
                // Walk the entire bitmask: process ALL non-ASCII bytes in this
                // chunk before advancing, avoiding redundant SIMD reloads.
                let mut mask = high_bits;
                let mut chunk_pos = 0usize;
                while mask != 0 {
                    let next_non_ascii = mask.trailing_zeros() as usize;
                    // Copy ASCII bytes between chunk_pos and next_non_ascii.
                    if next_non_ascii > chunk_pos {
                        // SAFETY: bytes in [i+chunk_pos .. i+next_non_ascii]
                        // have their high bit clear (0x00-0x7F), valid UTF-8.
                        let ascii_slice = core::str::from_utf8_unchecked(
                            &data[i + chunk_pos..i + next_non_ascii],
                        );
                        result.push_str(ascii_slice);
                    }
                    // Decode the non-ASCII byte via LUT.
                    result.push(CP1252_TABLE[data[i + next_non_ascii] as usize]);
                    chunk_pos = next_non_ascii + 1;
                    // Clear the lowest set bit.
                    mask &= mask - 1;
                }
                // Copy remaining ASCII bytes after the last non-ASCII byte.
                if chunk_pos < 32 {
                    // SAFETY: remaining bytes have high bit clear, valid UTF-8.
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i + chunk_pos..i + 32]);
                    result.push_str(ascii_slice);
                }
                i += 32;
            }
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
                    let ascii_slice = core::str::from_utf8_unchecked(&data[i..i + 16]);
                    result.push_str(ascii_slice);
                    i += 16;
                    continue;
                }
                // Walk the entire 16-bit bitmask: process ALL non-ASCII bytes
                // in this chunk before advancing.
                let mut mask = high_bits;
                let mut chunk_pos = 0usize;
                while mask != 0 {
                    let next_non_ascii = mask.trailing_zeros() as usize;
                    if next_non_ascii > chunk_pos {
                        // SAFETY: bytes in [i+chunk_pos .. i+next_non_ascii]
                        // have their high bit clear (0x00-0x7F), valid UTF-8.
                        let ascii_slice = core::str::from_utf8_unchecked(
                            &data[i + chunk_pos..i + next_non_ascii],
                        );
                        result.push_str(ascii_slice);
                    }
                    result.push(CP1252_TABLE[data[i + next_non_ascii] as usize]);
                    chunk_pos = next_non_ascii + 1;
                    mask &= mask - 1;
                }
                if chunk_pos < 16 {
                    // SAFETY: remaining bytes have high bit clear, valid UTF-8.
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i + chunk_pos..i + 16]);
                    result.push_str(ascii_slice);
                }
                i += 16;
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
    pub(super) unsafe fn decode_cp1252_sse2(data: &[u8]) -> String {
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
                    let ascii_slice = core::str::from_utf8_unchecked(&data[i..i + 16]);
                    result.push_str(ascii_slice);
                    i += 16;
                    continue;
                }
                // Walk the entire 16-bit bitmask: process ALL non-ASCII bytes
                // in this chunk before advancing.
                let mut mask = high_bits;
                let mut chunk_pos = 0usize;
                while mask != 0 {
                    let next_non_ascii = mask.trailing_zeros() as usize;
                    if next_non_ascii > chunk_pos {
                        // SAFETY: bytes in [i+chunk_pos .. i+next_non_ascii]
                        // have their high bit clear (0x00-0x7F), valid UTF-8.
                        let ascii_slice = core::str::from_utf8_unchecked(
                            &data[i + chunk_pos..i + next_non_ascii],
                        );
                        result.push_str(ascii_slice);
                    }
                    result.push(CP1252_TABLE[data[i + next_non_ascii] as usize]);
                    chunk_pos = next_non_ascii + 1;
                    mask &= mask - 1;
                }
                if chunk_pos < 16 {
                    // SAFETY: remaining bytes have high bit clear, valid UTF-8.
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i + chunk_pos..i + 16]);
                    result.push_str(ascii_slice);
                }
                i += 16;
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
    pub(super) unsafe fn decode_cp1252_neon(data: &[u8]) -> String {
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
                    let ascii_slice = core::str::from_utf8_unchecked(&data[i..i + 16]);
                    result.push_str(ascii_slice);
                    i += 16;
                    continue;
                }
                // Non-ASCII bytes found -- process ALL 16 bytes in this chunk
                // with a tight scalar loop, avoiding a SIMD reload.
                let mut j = 0usize;
                while j < 16 {
                    let b = data[i + j];
                    if b < 0x80 {
                        // Find the run of consecutive ASCII bytes.
                        let start = j;
                        j += 1;
                        while j < 16 && data[i + j] < 0x80 {
                            j += 1;
                        }
                        // SAFETY: bytes in [i+start .. i+j] are 0x00-0x7F,
                        // valid UTF-8.
                        let ascii_slice = core::str::from_utf8_unchecked(
                            &data[i + start..i + j],
                        );
                        result.push_str(ascii_slice);
                    } else {
                        result.push(CP1252_TABLE[b as usize]);
                        j += 1;
                    }
                }
                i += 16;
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
    #[allow(dead_code)]
    #[target_feature(enable = "simd128")]
    pub(super) unsafe fn decode_cp1252_simd128(data: &[u8]) -> String {
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
                    let ascii_slice = core::str::from_utf8_unchecked(&data[i..i + 16]);
                    result.push_str(ascii_slice);
                    i += 16;
                    continue;
                }
                // Walk the entire 16-bit bitmask: process ALL non-ASCII bytes
                // in this chunk before advancing.
                let mut mask = high_bits;
                let mut chunk_pos = 0usize;
                while mask != 0 {
                    let next_non_ascii = mask.trailing_zeros() as usize;
                    if next_non_ascii > chunk_pos {
                        // SAFETY: bytes in [i+chunk_pos .. i+next_non_ascii]
                        // have their high bit clear (0x00-0x7F), valid UTF-8.
                        let ascii_slice = core::str::from_utf8_unchecked(
                            &data[i + chunk_pos..i + next_non_ascii],
                        );
                        result.push_str(ascii_slice);
                    }
                    result.push(CP1252_TABLE[data[i + next_non_ascii] as usize]);
                    chunk_pos = next_non_ascii + 1;
                    mask &= mask - 1;
                }
                if chunk_pos < 16 {
                    // SAFETY: remaining bytes have high bit clear, valid UTF-8.
                    let ascii_slice =
                        core::str::from_utf8_unchecked(&data[i + chunk_pos..i + 16]);
                    result.push_str(ascii_slice);
                }
                i += 16;
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
#[inline]
pub(crate) fn decode_cp1252(data: &[u8]) -> String {
    if data.is_empty() {
        return String::new();
    }

    // Note: the SIMD decode paths (AVX2, SSE2, NEON) already handle ASCII
    // bytes at full throughput — they copy ASCII chunks directly and only fall
    // back to the LUT for non-ASCII bytes.  A separate `is_all_ascii` pre-scan
    // wastes a full buffer traversal when the input is not 100% ASCII (the
    // common case for CP-1252 text with a few special characters).

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if is_x86_feature_detected!("avx512bw") {
            // SAFETY: AVX512BW feature is confirmed present by the runtime check.
            return unsafe { x86::decode_cp1252_avx512bw(data) };
        }
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
            assert_eq!(got, expected, "mismatch for len={len}, data={data:?}");
        }
    }
}
