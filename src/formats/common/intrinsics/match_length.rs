//! Common prefix length comparison with SIMD acceleration.
//!
//! Provides [`common_prefix_length`], which returns the number of leading bytes
//! that are equal in two slices (up to a caller-supplied `max_len`).  The
//! dispatch function selects the best available SIMD backend at runtime:
//!
//! | Architecture | Backend        |
//! |-------------|----------------|
//! | x86 / x86_64 | AVX2 (32 B) then SSE2 (16 B) fallback |
//! | aarch64      | NEON (16 B)    |
//! | wasm32       | SIMD128 (16 B) |
//! | *other*      | scalar loop    |

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
    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn common_prefix_length_avx2(a: &[u8], b: &[u8], max_len: usize) -> usize {
        let limit = max_len.min(a.len()).min(b.len());
        let mut i: usize = 0;

        // --- 32-byte AVX2 chunks ---
        while i + 32 <= limit {
            // SAFETY: `i + 32 <= limit <= a.len()` and `i + 32 <= limit <= b.len()`,
            // so 32-byte unaligned loads are within bounds. AVX2 is enabled by
            // `target_feature`.
            unsafe {
                let va = _mm256_loadu_si256(a.as_ptr().add(i) as *const __m256i);
                let vb = _mm256_loadu_si256(b.as_ptr().add(i) as *const __m256i);
                let cmp = _mm256_cmpeq_epi8(va, vb);
                let mask = _mm256_movemask_epi8(cmp) as u32;
                if mask != 0xFFFF_FFFF {
                    return i + mask.trailing_ones() as usize;
                }
            }
            i += 32;
        }

        // --- 16-byte SSE2 tail ---
        if i + 16 <= limit {
            // SAFETY: `i + 16 <= limit <= a.len()` and same for `b`. SSE2 is
            // implied by AVX2.
            unsafe {
                let va = _mm_loadu_si128(a.as_ptr().add(i) as *const __m128i);
                let vb = _mm_loadu_si128(b.as_ptr().add(i) as *const __m128i);
                let cmp = _mm_cmpeq_epi8(va, vb);
                let mask = _mm_movemask_epi8(cmp) as u32;
                if mask != 0xFFFF {
                    return i + mask.trailing_ones() as usize;
                }
            }
            i += 16;
        }

        // --- scalar tail ---
        while i < limit && a[i] == b[i] {
            i += 1;
        }
        i
    }

    /// SSE2 implementation -- processes 16 bytes at a time, then a scalar tail.
    #[target_feature(enable = "sse2")]
    pub(super) unsafe fn common_prefix_length_sse2(a: &[u8], b: &[u8], max_len: usize) -> usize {
        let limit = max_len.min(a.len()).min(b.len());
        let mut i: usize = 0;

        // --- 16-byte SSE2 chunks ---
        while i + 16 <= limit {
            // SAFETY: `i + 16 <= limit <= a.len()` and same for `b`. SSE2 is
            // enabled by `target_feature`.
            unsafe {
                let va = _mm_loadu_si128(a.as_ptr().add(i) as *const __m128i);
                let vb = _mm_loadu_si128(b.as_ptr().add(i) as *const __m128i);
                let cmp = _mm_cmpeq_epi8(va, vb);
                let mask = _mm_movemask_epi8(cmp) as u32;
                if mask != 0xFFFF {
                    return i + mask.trailing_ones() as usize;
                }
            }
            i += 16;
        }

        // --- scalar tail ---
        while i < limit && a[i] == b[i] {
            i += 1;
        }
        i
    }
}

// ---------------------------------------------------------------------------
// aarch64  NEON implementation
// ---------------------------------------------------------------------------

#[cfg(target_arch = "aarch64")]
mod aarch64 {
    use core::arch::aarch64::*;

    /// NEON implementation -- processes 16 bytes at a time, then a scalar tail.
    ///
    /// NEON lacks a `movemask` equivalent, so we detect mismatches via
    /// `vminvq_u8` and locate the first one by extracting 64-bit halves and
    /// counting trailing zeros.
    pub(super) unsafe fn common_prefix_length_neon(a: &[u8], b: &[u8], max_len: usize) -> usize {
        let limit = max_len.min(a.len()).min(b.len());
        let mut i: usize = 0;

        while i + 16 <= limit {
            // SAFETY: `i + 16 <= limit <= a.len()` and same for `b`. NEON is
            // always available on aarch64.
            unsafe {
                let va = vld1q_u8(a.as_ptr().add(i));
                let vb = vld1q_u8(b.as_ptr().add(i));
                let eq = vceqq_u8(va, vb); // 0xFF where equal, 0x00 where different

                // Fast check: if all lanes are 0xFF, the minimum is 0xFF.
                if vminvq_u8(eq) != 0xFF {
                    // There is at least one mismatch. Invert so mismatches become
                    // non-zero, then find the first non-zero byte.
                    let neq = vmvnq_u8(eq);
                    let as_u64 = vreinterpretq_u64_u8(neq);
                    let lo = vgetq_lane_u64::<0>(as_u64);
                    if lo != 0 {
                        return i + (lo.trailing_zeros() / 8) as usize;
                    }
                    let hi = vgetq_lane_u64::<1>(as_u64);
                    return i + 8 + (hi.trailing_zeros() / 8) as usize;
                }
            }
            i += 16;
        }

        // --- scalar tail ---
        while i < limit && a[i] == b[i] {
            i += 1;
        }
        i
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
    pub(super) unsafe fn common_prefix_length_simd128(a: &[u8], b: &[u8], max_len: usize) -> usize {
        let limit = max_len.min(a.len()).min(b.len());
        let mut i: usize = 0;

        while i + 16 <= limit {
            // SAFETY: `i + 16 <= limit <= a.len()` and same for `b`. simd128 is
            // enabled by `target_feature`.
            unsafe {
                let va = v128_load(a.as_ptr().add(i) as *const v128);
                let vb = v128_load(b.as_ptr().add(i) as *const v128);
                let eq = i8x16_eq(va, vb);
                let mask = i8x16_bitmask(eq) as u32;
                if mask != 0xFFFF {
                    return i + mask.trailing_ones() as usize;
                }
            }
            i += 16;
        }

        // --- scalar tail ---
        while i < limit && a[i] == b[i] {
            i += 1;
        }
        i
    }
}

// ---------------------------------------------------------------------------
// Scalar fallback
// ---------------------------------------------------------------------------

/// Byte-by-byte common prefix length (portable fallback).
pub(crate) fn common_prefix_length_scalar(a: &[u8], b: &[u8], max_len: usize) -> usize {
    let limit = max_len.min(a.len()).min(b.len());
    let mut i: usize = 0;
    while i < limit && a[i] == b[i] {
        i += 1;
    }
    i
}

// ---------------------------------------------------------------------------
// Dispatch function
// ---------------------------------------------------------------------------

/// Returns the number of leading bytes that are equal in `a` and `b`, up to
/// `max_len`.
///
/// Selects the best available SIMD implementation at runtime.  The effective
/// comparison length is `max_len.min(a.len()).min(b.len())`.
#[allow(unreachable_code)]
pub(crate) fn common_prefix_length(a: &[u8], b: &[u8], max_len: usize) -> usize {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: AVX2 feature is confirmed present by the runtime check.
            return unsafe { x86::common_prefix_length_avx2(a, b, max_len) };
        }
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: SSE2 is always available on x86_64.
            return unsafe { x86::common_prefix_length_sse2(a, b, max_len) };
        }
        #[cfg(target_arch = "x86")]
        if is_x86_feature_detected!("sse2") {
            // SAFETY: SSE2 feature is confirmed present by the runtime check.
            return unsafe { x86::common_prefix_length_sse2(a, b, max_len) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64.
        return unsafe { aarch64::common_prefix_length_neon(a, b, max_len) };
    }
    #[cfg(target_arch = "wasm32")]
    {
        #[cfg(target_feature = "simd128")]
        {
            // SAFETY: simd128 target feature is statically enabled.
            return unsafe { wasm::common_prefix_length_simd128(a, b, max_len) };
        }
    }
    common_prefix_length_scalar(a, b, max_len)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_slices() {
        assert_eq!(common_prefix_length(b"", b"", 100), 0);
        assert_eq!(common_prefix_length(b"abc", b"", 100), 0);
        assert_eq!(common_prefix_length(b"", b"abc", 100), 0);
    }

    #[test]
    fn max_len_zero() {
        assert_eq!(common_prefix_length(b"abc", b"abc", 0), 0);
    }

    #[test]
    fn full_match() {
        assert_eq!(common_prefix_length(b"hello", b"hello", 5), 5);
        assert_eq!(common_prefix_length(b"hello", b"hello", 100), 5);
    }

    #[test]
    fn partial_match() {
        assert_eq!(common_prefix_length(b"abcXef", b"abcYef", 6), 3);
    }

    #[test]
    fn max_len_clamps() {
        assert_eq!(common_prefix_length(b"abcdef", b"abcdef", 3), 3);
    }

    #[test]
    fn max_len_exceeds_slice_len() {
        assert_eq!(common_prefix_length(b"ab", b"abcdef", 1000), 2);
        assert_eq!(common_prefix_length(b"abcdef", b"ab", 1000), 2);
    }

    #[test]
    fn mismatch_at_first_byte() {
        assert_eq!(common_prefix_length(b"Xbc", b"abc", 3), 0);
    }

    #[test]
    fn mismatch_at_last_byte() {
        assert_eq!(common_prefix_length(b"abX", b"abY", 3), 2);
    }

    #[test]
    fn exactly_16_bytes_all_match() {
        let data = b"0123456789abcdef";
        assert_eq!(data.len(), 16);
        assert_eq!(common_prefix_length(data, data, 16), 16);
    }

    #[test]
    fn exactly_16_bytes_mismatch_at_15() {
        let a = b"0123456789abcdeX";
        let b = b"0123456789abcdeY";
        assert_eq!(a.len(), 16);
        assert_eq!(common_prefix_length(a, b, 16), 15);
    }

    #[test]
    fn exactly_32_bytes_all_match() {
        let data = b"0123456789abcdef0123456789abcdef";
        assert_eq!(data.len(), 32);
        assert_eq!(common_prefix_length(data, data, 32), 32);
    }

    #[test]
    fn longer_than_32_with_mismatch() {
        let a = vec![0xAAu8; 50];
        let mut b = vec![0xAAu8; 50];
        b[37] = 0xBB; // mismatch at position 37
        assert_eq!(common_prefix_length(&a, &b, 50), 37);
        // Also verify scalar agrees.
        assert_eq!(common_prefix_length_scalar(&a, &b, 50), 37);
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

            let len_a = (rng % 200) as usize;
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let len_b = (rng % 200) as usize;
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let max_len = (rng % 200) as usize;

            // Generate slices that share a random common prefix then diverge.
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let common = (rng % (len_a.min(len_b).max(1) as u64)) as usize;

            let a: Vec<u8> = (0..len_a)
                .map(|i| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    if i < common {
                        (rng & 0x7F) as u8
                    } else {
                        (rng & 0xFF) as u8
                    }
                })
                .collect();

            let mut b: Vec<u8> = (0..len_b)
                .map(|i| {
                    if i < common {
                        a[i] // identical prefix
                    } else {
                        rng ^= rng << 13;
                        rng ^= rng >> 7;
                        rng ^= rng << 17;
                        (rng & 0xFF) as u8
                    }
                })
                .collect();

            // Force a mismatch at position `common` (if within bounds of both)
            // so the expected prefix is exactly `common` (unless max_len or
            // slice length is smaller).
            if common < len_a && common < len_b && a[common] == b[common] {
                b[common] = b[common].wrapping_add(1);
            }

            let expected = common_prefix_length_scalar(&a, &b, max_len);
            let got = common_prefix_length(&a, &b, max_len);
            assert_eq!(
                got, expected,
                "mismatch for len_a={len_a}, len_b={len_b}, max_len={max_len}, common={common}"
            );
        }
    }
}
