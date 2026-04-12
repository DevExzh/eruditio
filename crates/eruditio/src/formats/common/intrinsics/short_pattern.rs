//! Short pattern search (2-4 bytes) with SIMD acceleration.
//!
//! Provides [`find_short_pattern`], which finds the first occurrence of a 2-4
//! byte needle in a haystack. Uses SIMD to scan for the first byte of the
//! needle, then verifies the remaining 1-3 bytes at each candidate position.
//!
//! The dispatch function selects the best available SIMD backend at runtime:
//!
//! | Architecture  | Backend                                          |
//! |---------------|--------------------------------------------------|
//! | x86 / x86_64  | AVX512BW (64 B) > AVX2 (32 B) > SSE2 (16 B)    |
//! | aarch64       | NEON (16 B)                                      |
//! | wasm32        | SIMD128 (16 B)                                   |
//! | *other*       | scalar loop                                      |

// ---------------------------------------------------------------------------
// Shared verify helper
// ---------------------------------------------------------------------------

/// Verify that `haystack[candidate+1..candidate+needle_len]` matches
/// `needle[1..needle_len]`. Inlined so the compiler can unroll for small
/// needle lengths (2-4).
#[inline(always)]
fn verify_tail(haystack: &[u8], candidate: usize, needle: &[u8]) -> bool {
    let needle_len = needle.len();
    if candidate + needle_len > haystack.len() {
        return false;
    }
    // Compare remaining 1-3 bytes.
    haystack[candidate + 1..candidate + needle_len] == needle[1..needle_len]
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

    use super::verify_tail;

    /// AVX512BW implementation -- scans 64 bytes at a time for first-byte
    /// matches, then verifies remaining bytes at each candidate. Falls through
    /// to AVX2, SSE2, and scalar tails for the remaining bytes.
    #[target_feature(enable = "avx512bw")]
    pub(super) unsafe fn find_short_pattern_avx512bw(
        haystack: &[u8],
        needle: &[u8],
    ) -> Option<usize> {
        let len = haystack.len();
        let first = needle[0];
        let mut i: usize = 0;

        // SAFETY: AVX512BW is enabled by `target_feature`.
        unsafe {
            let splat = _mm512_set1_epi8(first as i8);

            // --- 64-byte AVX512BW chunks ---
            while i + 64 <= len {
                // SAFETY: `i + 64 <= len <= haystack.len()`, so the 64-byte
                // unaligned load is within bounds.
                let chunk = _mm512_loadu_si512(haystack.as_ptr().add(i) as *const __m512i);
                let mut mask = _mm512_cmpeq_epi8_mask(chunk, splat);

                while mask != 0 {
                    let bit_pos = mask.trailing_zeros() as usize;
                    let candidate = i + bit_pos;
                    if verify_tail(haystack, candidate, needle) {
                        return Some(candidate);
                    }
                    mask &= mask - 1; // clear lowest set bit
                }
                i += 64;
            }

            // --- 32-byte AVX2 tail ---
            if i + 32 <= len {
                let splat_256 = _mm256_set1_epi8(first as i8);
                let chunk = _mm256_loadu_si256(haystack.as_ptr().add(i) as *const __m256i);
                let cmp = _mm256_cmpeq_epi8(chunk, splat_256);
                let mut mask = _mm256_movemask_epi8(cmp) as u32;

                while mask != 0 {
                    let bit_pos = mask.trailing_zeros() as usize;
                    let candidate = i + bit_pos;
                    if verify_tail(haystack, candidate, needle) {
                        return Some(candidate);
                    }
                    mask &= mask - 1;
                }
                i += 32;
            }

            // --- 16-byte SSE2 tail ---
            if i + 16 <= len {
                let splat_128 = _mm_set1_epi8(first as i8);
                let chunk = _mm_loadu_si128(haystack.as_ptr().add(i) as *const __m128i);
                let cmp = _mm_cmpeq_epi8(chunk, splat_128);
                let mut mask = _mm_movemask_epi8(cmp) as u32;

                while mask != 0 {
                    let bit_pos = mask.trailing_zeros() as usize;
                    let candidate = i + bit_pos;
                    if verify_tail(haystack, candidate, needle) {
                        return Some(candidate);
                    }
                    mask &= mask - 1;
                }
                i += 16;
            }
        }

        // --- scalar tail ---
        while i + needle.len() <= len {
            if haystack[i] == first && verify_tail(haystack, i, needle) {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    /// AVX2 implementation -- scans 32 bytes at a time for first-byte matches,
    /// then verifies remaining bytes at each candidate.
    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn find_short_pattern_avx2(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        let len = haystack.len();
        let first = needle[0];
        let mut i: usize = 0;

        // SAFETY: AVX2 is enabled by `target_feature`.
        unsafe {
            let splat = _mm256_set1_epi8(first as i8);

            // --- 32-byte AVX2 chunks ---
            while i + 32 <= len {
                // SAFETY: `i + 32 <= len <= haystack.len()`, so the 32-byte
                // unaligned load is within bounds.
                let chunk = _mm256_loadu_si256(haystack.as_ptr().add(i) as *const __m256i);
                let cmp = _mm256_cmpeq_epi8(chunk, splat);
                let mut mask = _mm256_movemask_epi8(cmp) as u32;

                while mask != 0 {
                    let bit_pos = mask.trailing_zeros() as usize;
                    let candidate = i + bit_pos;
                    if verify_tail(haystack, candidate, needle) {
                        return Some(candidate);
                    }
                    mask &= mask - 1; // clear lowest set bit
                }
                i += 32;
            }

            // --- 16-byte SSE2 tail ---
            if i + 16 <= len {
                // SAFETY: `i + 16 <= len`. SSE2 is implied by AVX2.
                let splat_128 = _mm_set1_epi8(first as i8);
                let chunk = _mm_loadu_si128(haystack.as_ptr().add(i) as *const __m128i);
                let cmp = _mm_cmpeq_epi8(chunk, splat_128);
                let mut mask = _mm_movemask_epi8(cmp) as u32;

                while mask != 0 {
                    let bit_pos = mask.trailing_zeros() as usize;
                    let candidate = i + bit_pos;
                    if verify_tail(haystack, candidate, needle) {
                        return Some(candidate);
                    }
                    mask &= mask - 1;
                }
                i += 16;
            }
        }

        // --- scalar tail ---
        while i + needle.len() <= len {
            if haystack[i] == first && verify_tail(haystack, i, needle) {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    /// SSE2 implementation -- scans 16 bytes at a time.
    #[target_feature(enable = "sse2")]
    pub(super) unsafe fn find_short_pattern_sse2(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        let len = haystack.len();
        let first = needle[0];
        let mut i: usize = 0;

        // SAFETY: SSE2 is enabled by `target_feature`.
        unsafe {
            let splat = _mm_set1_epi8(first as i8);

            // --- 16-byte SSE2 chunks ---
            while i + 16 <= len {
                // SAFETY: `i + 16 <= len <= haystack.len()`. SSE2 is enabled by
                // `target_feature`.
                let chunk = _mm_loadu_si128(haystack.as_ptr().add(i) as *const __m128i);
                let cmp = _mm_cmpeq_epi8(chunk, splat);
                let mut mask = _mm_movemask_epi8(cmp) as u32;

                while mask != 0 {
                    let bit_pos = mask.trailing_zeros() as usize;
                    let candidate = i + bit_pos;
                    if verify_tail(haystack, candidate, needle) {
                        return Some(candidate);
                    }
                    mask &= mask - 1;
                }
                i += 16;
            }
        }

        // --- scalar tail ---
        while i + needle.len() <= len {
            if haystack[i] == first && verify_tail(haystack, i, needle) {
                return Some(i);
            }
            i += 1;
        }
        None
    }
}

// ---------------------------------------------------------------------------
// aarch64  NEON implementation
// ---------------------------------------------------------------------------

#[cfg(target_arch = "aarch64")]
mod aarch64 {
    use core::arch::aarch64::*;

    use super::verify_tail;

    /// NEON implementation -- scans 16 bytes at a time for first-byte matches.
    ///
    /// Extracts match positions from 64-bit lane halves and verifies each
    /// candidate. Processes all candidates within one chunk before advancing.
    pub(super) unsafe fn find_short_pattern_neon(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        let len = haystack.len();
        let first = needle[0];
        let mut i: usize = 0;

        // SAFETY: NEON is always available on aarch64.
        unsafe {
            let splat = vdupq_n_u8(first);

            while i + 16 <= len {
                // SAFETY: `i + 16 <= len <= haystack.len()`.
                let chunk = vld1q_u8(haystack.as_ptr().add(i));
                let matches = vceqq_u8(chunk, splat);

                // Quick exit: no matches in this chunk.
                if vmaxvq_u8(matches) == 0 {
                    i += 16;
                    continue;
                }

                // Extract u64 halves and iterate match positions.
                let as_u64 = vreinterpretq_u64_u8(matches);

                // Low 8 bytes (positions 0-7).
                let mut lo = vgetq_lane_u64::<0>(as_u64);
                while lo != 0 {
                    let byte_pos = (lo.trailing_zeros() / 8) as usize;
                    let candidate = i + byte_pos;
                    if verify_tail(haystack, candidate, needle) {
                        return Some(candidate);
                    }
                    // Clear this byte's 8 bits.
                    lo &= !(0xFF_u64 << (byte_pos * 8));
                }

                // High 8 bytes (positions 8-15).
                let mut hi = vgetq_lane_u64::<1>(as_u64);
                while hi != 0 {
                    let byte_pos = (hi.trailing_zeros() / 8) as usize;
                    let candidate = i + 8 + byte_pos;
                    if verify_tail(haystack, candidate, needle) {
                        return Some(candidate);
                    }
                    hi &= !(0xFF_u64 << (byte_pos * 8));
                }

                i += 16;
            }
        }

        // --- scalar tail ---
        while i + needle.len() <= len {
            if haystack[i] == first && verify_tail(haystack, i, needle) {
                return Some(i);
            }
            i += 1;
        }
        None
    }
}

// ---------------------------------------------------------------------------
// wasm32  SIMD128 implementation
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod wasm {
    use core::arch::wasm32::*;

    use super::verify_tail;

    /// SIMD128 implementation -- scans 16 bytes at a time.
    #[allow(dead_code)]
    #[target_feature(enable = "simd128")]
    pub(super) unsafe fn find_short_pattern_simd128(
        haystack: &[u8],
        needle: &[u8],
    ) -> Option<usize> {
        let len = haystack.len();
        let first = needle[0];
        let mut i: usize = 0;

        // SAFETY: simd128 is enabled by `target_feature`.
        unsafe {
            let splat = u8x16_splat(first);

            while i + 16 <= len {
                // SAFETY: `i + 16 <= len <= haystack.len()`.
                let chunk = v128_load(haystack.as_ptr().add(i) as *const v128);
                let cmp = i8x16_eq(chunk, splat);
                let mut mask = i8x16_bitmask(cmp) as u32;

                while mask != 0 {
                    let bit_pos = mask.trailing_zeros() as usize;
                    let candidate = i + bit_pos;
                    if verify_tail(haystack, candidate, needle) {
                        return Some(candidate);
                    }
                    mask &= mask - 1;
                }
                i += 16;
            }
        }

        // --- scalar tail ---
        while i + needle.len() <= len {
            if haystack[i] == first && verify_tail(haystack, i, needle) {
                return Some(i);
            }
            i += 1;
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Scalar fallback
// ---------------------------------------------------------------------------

/// Byte-by-byte sliding window search (portable fallback).
pub(crate) fn find_short_pattern_scalar(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------------------
// Dispatch function
// ---------------------------------------------------------------------------

/// Returns the index of the first occurrence of `needle` (2-4 bytes) in
/// `haystack`, or `None` if not found.
///
/// Panics in debug mode if `needle.len() < 2 || needle.len() > 4`.
/// In release mode, needles outside this range fall through to scalar.
///
/// Selects the best available SIMD implementation at runtime.
#[allow(unreachable_code)]
#[inline]
pub(crate) fn find_short_pattern(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    debug_assert!(
        needle.len() >= 2 && needle.len() <= 4,
        "find_short_pattern: needle must be 2-4 bytes, got {}",
        needle.len()
    );

    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }

    // Fall through to scalar for out-of-range needles in release mode.
    if needle.len() < 2 || needle.len() > 4 {
        return find_short_pattern_scalar(haystack, needle);
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if is_x86_feature_detected!("avx512bw") {
            // SAFETY: AVX512BW feature is confirmed present by the runtime check.
            return unsafe { x86::find_short_pattern_avx512bw(haystack, needle) };
        }
        if is_x86_feature_detected!("avx2") {
            // SAFETY: AVX2 feature is confirmed present by the runtime check.
            return unsafe { x86::find_short_pattern_avx2(haystack, needle) };
        }
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: SSE2 is always available on x86_64.
            return unsafe { x86::find_short_pattern_sse2(haystack, needle) };
        }
        #[cfg(target_arch = "x86")]
        if is_x86_feature_detected!("sse2") {
            // SAFETY: SSE2 feature is confirmed present by the runtime check.
            return unsafe { x86::find_short_pattern_sse2(haystack, needle) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64.
        return unsafe { aarch64::find_short_pattern_neon(haystack, needle) };
    }
    #[cfg(target_arch = "wasm32")]
    {
        #[cfg(target_feature = "simd128")]
        {
            // SAFETY: simd128 target feature is statically enabled.
            return unsafe { wasm::find_short_pattern_simd128(haystack, needle) };
        }
    }
    find_short_pattern_scalar(haystack, needle)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_haystack() {
        assert_eq!(find_short_pattern(b"", b"</"), None);
    }

    #[test]
    fn needle_longer_than_haystack() {
        assert_eq!(find_short_pattern(b"x", b"</"), None);
    }

    #[test]
    fn match_at_start_2b() {
        assert_eq!(find_short_pattern(b"</html>", b"</"), Some(0));
    }

    #[test]
    fn match_at_end_2b() {
        assert_eq!(find_short_pattern(b"hello</", b"</"), Some(5));
    }

    #[test]
    fn match_in_middle_2b() {
        assert_eq!(find_short_pattern(b"abc</def", b"</"), Some(3));
    }

    #[test]
    fn no_match_2b() {
        assert_eq!(find_short_pattern(b"hello world", b"</"), None);
    }

    #[test]
    fn match_4b() {
        assert_eq!(
            find_short_pattern(b"text<!--comment-->more", b"<!--"),
            Some(4)
        );
    }

    #[test]
    fn match_3b() {
        assert_eq!(find_short_pattern(b"foo-->bar", b"-->"), Some(3));
    }

    #[test]
    fn match_at_register_boundary_15() {
        let mut data = vec![b'x'; 32];
        data[14] = b'<';
        data[15] = b'/';
        assert_eq!(find_short_pattern(&data, b"</"), Some(14));
    }

    #[test]
    fn match_at_register_boundary_16() {
        let mut data = vec![b'x'; 32];
        data[16] = b'<';
        data[17] = b'/';
        assert_eq!(find_short_pattern(&data, b"</"), Some(16));
    }

    #[test]
    fn match_at_register_boundary_31() {
        let mut data = vec![b'x'; 34];
        data[31] = b'<';
        data[32] = b'/';
        assert_eq!(find_short_pattern(&data, b"</"), Some(31));
    }

    #[test]
    fn first_byte_matches_but_second_does_not() {
        // '<' appears but is not followed by '/'
        assert_eq!(find_short_pattern(b"<p>text<b>more", b"</"), None);
    }

    #[test]
    fn multiple_matches_returns_first() {
        assert_eq!(find_short_pattern(b"</a></b></c>", b"</"), Some(0));
    }

    #[test]
    fn overlapping_pattern() {
        assert_eq!(find_short_pattern(b"<<<//", b"</"), Some(2));
    }

    #[test]
    fn exactly_16_bytes() {
        let data = b"0123456789ab</ef";
        assert_eq!(data.len(), 16);
        assert_eq!(find_short_pattern(data, b"</"), Some(12));
    }

    #[test]
    fn exactly_32_bytes() {
        let data = b"0123456789abcdef01234567</abcdef";
        assert_eq!(data.len(), 32);
        assert_eq!(find_short_pattern(data, b"</"), Some(24));
    }

    #[test]
    fn property_simd_matches_scalar() {
        let mut rng: u64 = 0xCAFE_BABE_DEAD_BEEF;

        for _ in 0..2000 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;

            let hay_len = (rng % 200) as usize;
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let needle_len = ((rng % 3) + 2) as usize; // 2, 3, or 4

            let haystack: Vec<u8> = (0..hay_len)
                .map(|_| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    (rng & 0xFF) as u8
                })
                .collect();

            let needle: Vec<u8> = (0..needle_len)
                .map(|_| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    (rng & 0xFF) as u8
                })
                .collect();

            let expected = find_short_pattern_scalar(&haystack, &needle);
            let got = find_short_pattern(&haystack, &needle);
            assert_eq!(
                got, expected,
                "mismatch for hay_len={hay_len}, needle={needle:?}"
            );
        }
    }
}
