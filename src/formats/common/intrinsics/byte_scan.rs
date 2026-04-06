//! Byte-in-set scanning with SIMD acceleration.
//!
//! Provides [`find_first_in_set`] and [`has_any_in_set`], which locate the first
//! byte in `haystack` that belongs to a caller-supplied `set`.  This fills the
//! gap where `memchr` handles up to 3 needles -- HTML/XML escape needs 5
//! (`&<>"'`).
//!
//! The dispatch function selects the best available SIMD backend at runtime:
//!
//! | Architecture  | Backend        |
//! |--------------|----------------|
//! | x86 / x86_64 | AVX2 (32 B) then SSE2 (16 B) fallback |
//! | aarch64       | NEON (16 B)    |
//! | wasm32        | SIMD128 (16 B) |
//! | *other*       | scalar loop    |

// ---------------------------------------------------------------------------
// Nibble lookup table builder (for pshufb set-membership technique)
// ---------------------------------------------------------------------------

/// Build 16-byte nibble lookup tables for `pshufb`-based set membership.
///
/// For each set member, its low nibble indexes into `lo_table` and its high
/// nibble indexes into `hi_table`.  Each member gets a unique bit (up to 8).
/// A byte `b` is in the set iff `lo_table[b & 0x0F] & hi_table[b >> 4] != 0`.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[inline]
fn build_nibble_luts(set: &[u8]) -> ([u8; 16], [u8; 16]) {
    let mut lo = [0u8; 16];
    let mut hi = [0u8; 16];
    let mut i = 0;
    while i < set.len() && i < 8 {
        let bit = 1u8 << i;
        lo[(set[i] & 0x0F) as usize] |= bit;
        hi[((set[i] >> 4) & 0x0F) as usize] |= bit;
        i += 1;
    }
    (lo, hi)
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

    /// AVX2 implementation -- processes 32 bytes at a time, then a 16-byte
    /// SSE2 tail, then a scalar tail.
    ///
    /// For sets of 4-8 bytes, delegates to a `pshufb` nibble-split path
    /// that is O(1) per chunk regardless of set size (simdjson technique).
    /// For sets of 1-3 bytes, the per-needle `cmpeq` loop is faster.
    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn find_first_in_set_avx2(haystack: &[u8], set: &[u8]) -> Option<usize> {
        if set.len() >= 4 && set.len() <= 8 {
            // SAFETY: AVX2 implies SSSE3 at runtime; the target_feature on
            // the callee satisfies the compiler's static requirement.
            return unsafe { find_first_in_set_avx2_pshufb(haystack, set) };
        }

        let len = haystack.len();
        let mut i: usize = 0;

        // --- 32-byte AVX2 chunks ---
        while i + 32 <= len {
            unsafe {
                let chunk = _mm256_loadu_si256(haystack.as_ptr().add(i) as *const __m256i);
                let mut combined = _mm256_setzero_si256();
                for &needle in set {
                    let splat = _mm256_set1_epi8(needle as i8);
                    let cmp = _mm256_cmpeq_epi8(chunk, splat);
                    combined = _mm256_or_si256(combined, cmp);
                }
                let mask = _mm256_movemask_epi8(combined) as u32;
                if mask != 0 {
                    return Some(i + mask.trailing_zeros() as usize);
                }
            }
            i += 32;
        }

        // --- 16-byte SSE2 tail ---
        if i + 16 <= len {
            unsafe {
                let chunk = _mm_loadu_si128(haystack.as_ptr().add(i) as *const __m128i);
                let mut combined = _mm_setzero_si128();
                for &needle in set {
                    let splat = _mm_set1_epi8(needle as i8);
                    let cmp = _mm_cmpeq_epi8(chunk, splat);
                    combined = _mm_or_si128(combined, cmp);
                }
                let mask = _mm_movemask_epi8(combined) as u32;
                if mask != 0 {
                    return Some(i + mask.trailing_zeros() as usize);
                }
            }
            i += 16;
        }

        // --- scalar tail ---
        while i < len {
            if set.contains(&haystack[i]) {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    /// AVX2 + SSSE3 pshufb nibble-split path for sets of 4-8 bytes.
    ///
    /// Builds two 16-byte lookup tables (one for low nibbles, one for high
    /// nibbles) and checks membership with 2 `vpshufb` + 1 `vpand` per
    /// 32-byte chunk — O(1) regardless of set size.
    #[target_feature(enable = "avx2,ssse3")]
    unsafe fn find_first_in_set_avx2_pshufb(haystack: &[u8], set: &[u8]) -> Option<usize> {
        let (lo_lut, hi_lut) = super::build_nibble_luts(set);
        let len = haystack.len();
        let mut i: usize = 0;

        unsafe {
            // Broadcast 128-bit LUTs to both AVX2 lanes.
            let lo128 = _mm_loadu_si128(lo_lut.as_ptr() as *const __m128i);
            let hi128 = _mm_loadu_si128(hi_lut.as_ptr() as *const __m128i);
            let lo256 = _mm256_broadcastsi128_si256(lo128);
            let hi256 = _mm256_broadcastsi128_si256(hi128);
            let mask_0f_256 = _mm256_set1_epi8(0x0F);
            let zero256 = _mm256_setzero_si256();

            // --- 32-byte AVX2 chunks ---
            while i + 32 <= len {
                let chunk = _mm256_loadu_si256(haystack.as_ptr().add(i) as *const __m256i);
                let lo_nib = _mm256_and_si256(chunk, mask_0f_256);
                let hi_nib = _mm256_and_si256(_mm256_srli_epi16(chunk, 4), mask_0f_256);
                let lo_match = _mm256_shuffle_epi8(lo256, lo_nib);
                let hi_match = _mm256_shuffle_epi8(hi256, hi_nib);
                let matched = _mm256_and_si256(lo_match, hi_match);

                // Fast path: testz returns 1 when matched is all-zero (no match).
                if _mm256_testz_si256(matched, matched) == 0 {
                    let zero_mask =
                        _mm256_movemask_epi8(_mm256_cmpeq_epi8(matched, zero256)) as u32;
                    return Some(i + (!zero_mask).trailing_zeros() as usize);
                }
                i += 32;
            }

            // --- 16-byte SSSE3 tail ---
            let mask_0f_128 = _mm_set1_epi8(0x0F);
            let zero128 = _mm_setzero_si128();

            if i + 16 <= len {
                let chunk = _mm_loadu_si128(haystack.as_ptr().add(i) as *const __m128i);
                let lo_nib = _mm_and_si128(chunk, mask_0f_128);
                let hi_nib = _mm_and_si128(_mm_srli_epi16(chunk, 4), mask_0f_128);
                let lo_match = _mm_shuffle_epi8(lo128, lo_nib);
                let hi_match = _mm_shuffle_epi8(hi128, hi_nib);
                let matched = _mm_and_si128(lo_match, hi_match);

                let zero_mask = _mm_movemask_epi8(_mm_cmpeq_epi8(matched, zero128)) as u32;
                if zero_mask != 0xFFFF {
                    return Some(i + ((!zero_mask) & 0xFFFF).trailing_zeros() as usize);
                }
                i += 16;
            }
        }

        // --- scalar tail ---
        while i < len {
            if set.contains(&haystack[i]) {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    /// SSE2 implementation -- processes 16 bytes at a time, then a scalar tail.
    #[target_feature(enable = "sse2")]
    pub(super) unsafe fn find_first_in_set_sse2(haystack: &[u8], set: &[u8]) -> Option<usize> {
        let len = haystack.len();
        let mut i: usize = 0;

        // --- 16-byte SSE2 chunks ---
        while i + 16 <= len {
            // SAFETY: `i + 16 <= len <= haystack.len()`. SSE2 is enabled by
            // `target_feature`.
            unsafe {
                let chunk = _mm_loadu_si128(haystack.as_ptr().add(i) as *const __m128i);
                let mut combined = _mm_setzero_si128();
                for &needle in set {
                    let splat = _mm_set1_epi8(needle as i8);
                    let cmp = _mm_cmpeq_epi8(chunk, splat);
                    combined = _mm_or_si128(combined, cmp);
                }
                let mask = _mm_movemask_epi8(combined) as u32;
                if mask != 0 {
                    return Some(i + mask.trailing_zeros() as usize);
                }
            }
            i += 16;
        }

        // --- scalar tail ---
        while i < len {
            if set.contains(&haystack[i]) {
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

    /// NEON implementation -- processes 16 bytes at a time, then a scalar tail.
    pub(super) unsafe fn find_first_in_set_neon(haystack: &[u8], set: &[u8]) -> Option<usize> {
        let len = haystack.len();
        let mut i: usize = 0;

        while i + 16 <= len {
            // SAFETY: `i + 16 <= len <= haystack.len()`. NEON is always
            // available on aarch64.
            unsafe {
                let chunk = vld1q_u8(haystack.as_ptr().add(i));
                let mut matches = vdupq_n_u8(0);
                for &needle in set {
                    let splat = vdupq_n_u8(needle);
                    let cmp = vceqq_u8(chunk, splat);
                    matches = vorrq_u8(matches, cmp);
                }
                // Fast check: if any lane is non-zero, the max is non-zero.
                if vmaxvq_u8(matches) != 0 {
                    // Find the position of the first non-zero byte.
                    let as_u64 = vreinterpretq_u64_u8(matches);
                    let lo = vgetq_lane_u64::<0>(as_u64);
                    if lo != 0 {
                        return Some(i + (lo.trailing_zeros() / 8) as usize);
                    }
                    let hi = vgetq_lane_u64::<1>(as_u64);
                    return Some(i + 8 + (hi.trailing_zeros() / 8) as usize);
                }
            }
            i += 16;
        }

        // --- scalar tail ---
        while i < len {
            if set.contains(&haystack[i]) {
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

    /// SIMD128 implementation -- processes 16 bytes at a time, then a scalar
    /// tail.
    #[allow(dead_code)]
    #[target_feature(enable = "simd128")]
    pub(super) unsafe fn find_first_in_set_simd128(haystack: &[u8], set: &[u8]) -> Option<usize> {
        let len = haystack.len();
        let mut i: usize = 0;

        while i + 16 <= len {
            // SAFETY: `i + 16 <= len <= haystack.len()`. simd128 is enabled by
            // `target_feature`.
            unsafe {
                let chunk = v128_load(haystack.as_ptr().add(i) as *const v128);
                let mut combined = u8x16_splat(0);
                for &needle in set {
                    let splat = u8x16_splat(needle);
                    let cmp = i8x16_eq(chunk, splat);
                    combined = v128_or(combined, cmp);
                }
                let mask = i8x16_bitmask(combined) as u32;
                if mask != 0 {
                    return Some(i + mask.trailing_zeros() as usize);
                }
            }
            i += 16;
        }

        // --- scalar tail ---
        while i < len {
            if set.contains(&haystack[i]) {
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

/// Byte-by-byte scan for the first byte in `haystack` that belongs to `set`
/// (portable fallback).
pub(crate) fn find_first_in_set_scalar(haystack: &[u8], set: &[u8]) -> Option<usize> {
    haystack.iter().position(|b| set.contains(b))
}

// ---------------------------------------------------------------------------
// Dispatch functions
// ---------------------------------------------------------------------------

/// Returns the index of the first byte in `haystack` that belongs to `set`,
/// or `None` if no such byte exists.
///
/// Selects the best available SIMD implementation at runtime.
#[allow(unreachable_code)]
#[inline]
pub(crate) fn find_first_in_set(haystack: &[u8], set: &[u8]) -> Option<usize> {
    if set.is_empty() || haystack.is_empty() {
        return None;
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: AVX2 feature is confirmed present by the runtime check.
            return unsafe { x86::find_first_in_set_avx2(haystack, set) };
        }
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: SSE2 is always available on x86_64.
            return unsafe { x86::find_first_in_set_sse2(haystack, set) };
        }
        #[cfg(target_arch = "x86")]
        if is_x86_feature_detected!("sse2") {
            // SAFETY: SSE2 feature is confirmed present by the runtime check.
            return unsafe { x86::find_first_in_set_sse2(haystack, set) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64.
        return unsafe { aarch64::find_first_in_set_neon(haystack, set) };
    }
    #[cfg(target_arch = "wasm32")]
    {
        #[cfg(target_feature = "simd128")]
        {
            // SAFETY: simd128 target feature is statically enabled.
            return unsafe { wasm::find_first_in_set_simd128(haystack, set) };
        }
    }
    find_first_in_set_scalar(haystack, set)
}

/// Returns `true` if any byte in `haystack` belongs to `set`.
#[cfg(test)]
pub(crate) fn has_any_in_set(haystack: &[u8], set: &[u8]) -> bool {
    find_first_in_set(haystack, set).is_some()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_haystack() {
        assert_eq!(find_first_in_set(b"", b"abc"), None);
    }

    #[test]
    fn empty_set() {
        assert_eq!(find_first_in_set(b"hello", b""), None);
    }

    #[test]
    fn no_match() {
        assert_eq!(find_first_in_set(b"hello world", b"xyz"), None);
    }

    #[test]
    fn match_at_start() {
        assert_eq!(find_first_in_set(b"&hello", b"&<>"), Some(0));
    }

    #[test]
    fn match_at_end() {
        assert_eq!(find_first_in_set(b"hello&", b"&<>"), Some(5));
    }

    #[test]
    fn match_in_middle() {
        assert_eq!(find_first_in_set(b"hel<lo", b"&<>"), Some(3));
    }

    #[test]
    fn xml_escape_set() {
        let set = b"&<>\"'";

        // No special chars
        assert_eq!(find_first_in_set(b"plain text here", set), None);

        // Ampersand first
        assert_eq!(find_first_in_set(b"foo & bar", set), Some(4));

        // Quote in attribute
        assert_eq!(find_first_in_set(b"value=\"test\"", set), Some(6));

        // Apostrophe
        assert_eq!(find_first_in_set(b"it's here", set), Some(2));

        // Multiple specials -- should find the first one
        assert_eq!(find_first_in_set(b"a<b>c&d", set), Some(1));
    }

    #[test]
    fn has_any_delegates() {
        assert!(has_any_in_set(b"hello & world", b"&<>"));
        assert!(!has_any_in_set(b"hello world", b"&<>"));
        assert!(!has_any_in_set(b"", b"&"));
        assert!(!has_any_in_set(b"abc", b""));
    }

    #[test]
    fn short_input_under_16() {
        assert_eq!(find_first_in_set(b"tiny", b"y"), Some(3));
        assert_eq!(find_first_in_set(b"tiny", b"z"), None);
        assert_eq!(find_first_in_set(b"a", b"a"), Some(0));
    }

    #[test]
    fn exactly_16_bytes() {
        let data = b"0123456789abcdef";
        assert_eq!(data.len(), 16);
        assert_eq!(find_first_in_set(data, b"f"), Some(15));
        assert_eq!(find_first_in_set(data, b"0"), Some(0));
        assert_eq!(find_first_in_set(data, b"z"), None);
    }

    #[test]
    fn exactly_32_bytes() {
        let data = b"0123456789abcdef0123456789abcdef";
        assert_eq!(data.len(), 32);
        assert_eq!(find_first_in_set(data, b"f"), Some(15));
        assert_eq!(find_first_in_set(data, b"0"), Some(0));
        assert_eq!(find_first_in_set(data, b"z"), None);
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

            let hay_len = (rng % 200) as usize;
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let set_len = ((rng % 10) + 1) as usize;

            let haystack: Vec<u8> = (0..hay_len)
                .map(|_| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    (rng & 0xFF) as u8
                })
                .collect();

            let set: Vec<u8> = (0..set_len)
                .map(|_| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    (rng & 0xFF) as u8
                })
                .collect();

            let expected = find_first_in_set_scalar(&haystack, &set);
            let got = find_first_in_set(&haystack, &set);
            assert_eq!(
                got, expected,
                "mismatch for hay_len={hay_len}, set_len={set_len}, set={set:?}"
            );
        }
    }
}
