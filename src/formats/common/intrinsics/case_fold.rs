//! Case-insensitive ASCII comparison with SIMD acceleration.
//!
//! Provides [`eq_ignore_ascii_case`], which returns `true` if two byte slices
//! are equal when ASCII letters are folded to lowercase.  Non-ASCII bytes are
//! compared exactly (byte-identical).
//!
//! The dispatch function selects the best available SIMD backend at runtime:
//!
//! | Architecture  | Backend                                |
//! |---------------|----------------------------------------|
//! | x86 / x86_64  | AVX2 (32 B) then SSE2 (16 B) fallback |
//! | aarch64       | NEON (16 B)                            |
//! | wasm32        | SIMD128 (16 B)                         |
//! | *other*       | scalar loop                            |

// ---------------------------------------------------------------------------
// x86 / x86_64  SIMD implementations
// ---------------------------------------------------------------------------

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86 {
    #[cfg(target_arch = "x86")]
    use core::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use core::arch::x86_64::*;

    /// Lowercase ASCII letters in a 128-bit vector by OR-ing 0x20 into bytes
    /// that fall in the A-Z range.
    #[inline(always)]
    unsafe fn to_lower_sse2(v: __m128i) -> __m128i {
        // SAFETY: caller guarantees SSE2 is available.
        unsafe {
            let a_minus_1 = _mm_set1_epi8(b'A' as i8 - 1); // 0x40
            let z_plus_1 = _mm_set1_epi8(b'Z' as i8 + 1); // 0x5B
            let mask_20 = _mm_set1_epi8(0x20);
            // is_upper = (v > 'A'-1) & (v < 'Z'+1)
            let gt_a = _mm_cmpgt_epi8(v, a_minus_1);
            let lt_z = _mm_cmplt_epi8(v, z_plus_1);
            let is_upper = _mm_and_si128(gt_a, lt_z);
            // result = v | (is_upper & 0x20)
            _mm_or_si128(v, _mm_and_si128(is_upper, mask_20))
        }
    }

    /// Lowercase ASCII letters in a 256-bit vector by OR-ing 0x20 into bytes
    /// that fall in the A-Z range.
    #[inline(always)]
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    unsafe fn to_lower_avx2(v: __m256i) -> __m256i {
        // SAFETY: caller guarantees AVX2 is available.
        unsafe {
            let a_minus_1 = _mm256_set1_epi8(b'A' as i8 - 1); // 0x40
            let z_plus_1 = _mm256_set1_epi8(b'Z' as i8 + 1); // 0x5B
            let mask_20 = _mm256_set1_epi8(0x20);
            // is_upper = (v > 'A'-1) & (z_plus_1 > v)
            // Note: _mm256_cmplt_epi8 does not exist, so we reverse the operands.
            let gt_a = _mm256_cmpgt_epi8(v, a_minus_1);
            let lt_z = _mm256_cmpgt_epi8(z_plus_1, v);
            let is_upper = _mm256_and_si256(gt_a, lt_z);
            // result = v | (is_upper & 0x20)
            _mm256_or_si256(v, _mm256_and_si256(is_upper, mask_20))
        }
    }

    /// AVX2 implementation -- processes 32 bytes at a time with case folding,
    /// then a 16-byte SSE2 tail, then a scalar tail.
    #[target_feature(enable = "avx2")]
    pub(crate) unsafe fn eq_ignore_ascii_case_avx2(a: &[u8], b: &[u8]) -> bool {
        debug_assert_eq!(a.len(), b.len());
        let len = a.len();
        let mut i: usize = 0;

        // --- 32-byte AVX2 chunks ---
        while i + 32 <= len {
            // SAFETY: `i + 32 <= len == a.len() == b.len()`, so 32-byte
            // unaligned loads are within bounds. AVX2 is enabled by
            // `target_feature`.
            unsafe {
                let va = _mm256_loadu_si256(a.as_ptr().add(i) as *const __m256i);
                let vb = _mm256_loadu_si256(b.as_ptr().add(i) as *const __m256i);
                let la = to_lower_avx2(va);
                let lb = to_lower_avx2(vb);
                let cmp = _mm256_cmpeq_epi8(la, lb);
                let mask = _mm256_movemask_epi8(cmp) as u32;
                if mask != 0xFFFF_FFFF {
                    return false;
                }
            }
            i += 32;
        }

        // --- 16-byte SSE2 tail ---
        if i + 16 <= len {
            // SAFETY: `i + 16 <= len == a.len() == b.len()`. SSE2 is implied
            // by AVX2.
            unsafe {
                let va = _mm_loadu_si128(a.as_ptr().add(i) as *const __m128i);
                let vb = _mm_loadu_si128(b.as_ptr().add(i) as *const __m128i);
                let la = to_lower_sse2(va);
                let lb = to_lower_sse2(vb);
                let cmp = _mm_cmpeq_epi8(la, lb);
                let mask = _mm_movemask_epi8(cmp) as u32;
                if mask != 0xFFFF {
                    return false;
                }
            }
            i += 16;
        }

        // --- scalar tail ---
        while i < len {
            if !a[i].eq_ignore_ascii_case(&b[i]) {
                return false;
            }
            i += 1;
        }
        true
    }

    /// SSE2 implementation -- processes 16 bytes at a time with case folding,
    /// then a scalar tail.
    #[target_feature(enable = "sse2")]
    pub(crate) unsafe fn eq_ignore_ascii_case_sse2(a: &[u8], b: &[u8]) -> bool {
        debug_assert_eq!(a.len(), b.len());
        let len = a.len();
        let mut i: usize = 0;

        // --- 16-byte SSE2 chunks ---
        while i + 16 <= len {
            // SAFETY: `i + 16 <= len == a.len() == b.len()`, so 16-byte
            // unaligned loads are within bounds. SSE2 is enabled by
            // `target_feature`.
            unsafe {
                let va = _mm_loadu_si128(a.as_ptr().add(i) as *const __m128i);
                let vb = _mm_loadu_si128(b.as_ptr().add(i) as *const __m128i);
                let la = to_lower_sse2(va);
                let lb = to_lower_sse2(vb);
                let cmp = _mm_cmpeq_epi8(la, lb);
                let mask = _mm_movemask_epi8(cmp) as u32;
                if mask != 0xFFFF {
                    return false;
                }
            }
            i += 16;
        }

        // --- scalar tail ---
        while i < len {
            if !a[i].eq_ignore_ascii_case(&b[i]) {
                return false;
            }
            i += 1;
        }
        true
    }
}

// ---------------------------------------------------------------------------
// aarch64  NEON implementation
// ---------------------------------------------------------------------------

#[cfg(target_arch = "aarch64")]
mod aarch64 {
    use core::arch::aarch64::*;

    /// NEON implementation -- processes 16 bytes at a time with case folding,
    /// then a scalar tail.
    pub(crate) unsafe fn eq_ignore_ascii_case_neon(a: &[u8], b: &[u8]) -> bool {
        debug_assert_eq!(a.len(), b.len());
        let len = a.len();
        let mut i: usize = 0;

        let mask_20 = vdupq_n_u8(0x20);
        let upper_a = vdupq_n_u8(b'A');
        let range = vdupq_n_u8(26); // 'Z' - 'A' + 1

        while i + 16 <= len {
            // SAFETY: `i + 16 <= len == a.len() == b.len()`, so 16-byte
            // loads are within bounds. NEON is always available on aarch64.
            unsafe {
                let va = vld1q_u8(a.as_ptr().add(i));
                let vb = vld1q_u8(b.as_ptr().add(i));
                // Lowercase: if (v - 'A') < 26, OR with 0x20
                let la = vorrq_u8(
                    va,
                    vandq_u8(vcltq_u8(vsubq_u8(va, upper_a), range), mask_20),
                );
                let lb = vorrq_u8(
                    vb,
                    vandq_u8(vcltq_u8(vsubq_u8(vb, upper_a), range), mask_20),
                );
                // All bytes equal?
                if vminvq_u8(vceqq_u8(la, lb)) != 0xFF {
                    return false;
                }
            }
            i += 16;
        }

        // --- scalar tail ---
        while i < len {
            if !a[i].eq_ignore_ascii_case(&b[i]) {
                return false;
            }
            i += 1;
        }
        true
    }
}

// ---------------------------------------------------------------------------
// wasm32  SIMD128 implementation
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod wasm {
    use core::arch::wasm32::*;

    /// SIMD128 implementation -- processes 16 bytes at a time with case folding,
    /// then a scalar tail.
    #[target_feature(enable = "simd128")]
    pub(crate) unsafe fn eq_ignore_ascii_case_simd128(a: &[u8], b: &[u8]) -> bool {
        debug_assert_eq!(a.len(), b.len());
        let len = a.len();
        let mut i: usize = 0;

        let upper_a = u8x16_splat(b'A');
        let range = u8x16_splat(26);
        let mask_20 = u8x16_splat(0x20);

        while i + 16 <= len {
            // SAFETY: `i + 16 <= len == a.len() == b.len()`, so 16-byte
            // loads are within bounds. simd128 is enabled by `target_feature`.
            unsafe {
                let va = v128_load(a.as_ptr().add(i) as *const v128);
                let vb = v128_load(b.as_ptr().add(i) as *const v128);
                // Lowercase: if (v - 'A') < 26, OR with 0x20
                let la = v128_or(
                    va,
                    v128_and(u8x16_lt(u8x16_sub(va, upper_a), range), mask_20),
                );
                let lb = v128_or(
                    vb,
                    v128_and(u8x16_lt(u8x16_sub(vb, upper_a), range), mask_20),
                );
                if i8x16_bitmask(i8x16_eq(la, lb)) != 0xFFFF_u16 as i32 {
                    return false;
                }
            }
            i += 16;
        }

        // --- scalar tail ---
        while i < len {
            if !a[i].eq_ignore_ascii_case(&b[i]) {
                return false;
            }
            i += 1;
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Scalar fallback
// ---------------------------------------------------------------------------

/// Byte-by-byte case-insensitive ASCII comparison (portable fallback).
pub(crate) fn eq_ignore_ascii_case_scalar(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

// ---------------------------------------------------------------------------
// Dispatch function
// ---------------------------------------------------------------------------

/// Returns `true` if `a` and `b` are equal when ASCII letters are folded to
/// lowercase.  Non-ASCII bytes must be byte-identical.
///
/// Selects the best available SIMD implementation at runtime.
#[allow(unreachable_code)]
pub(crate) fn eq_ignore_ascii_case(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: AVX2 feature is confirmed present by the runtime check.
            return unsafe { x86::eq_ignore_ascii_case_avx2(a, b) };
        }
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: SSE2 is always available on x86_64.
            return unsafe { x86::eq_ignore_ascii_case_sse2(a, b) };
        }
        #[cfg(target_arch = "x86")]
        if is_x86_feature_detected!("sse2") {
            // SAFETY: SSE2 feature is confirmed present by the runtime check.
            return unsafe { x86::eq_ignore_ascii_case_sse2(a, b) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64.
        return unsafe { aarch64::eq_ignore_ascii_case_neon(a, b) };
    }
    #[cfg(target_arch = "wasm32")]
    {
        #[cfg(target_feature = "simd128")]
        {
            // SAFETY: simd128 target feature is statically enabled.
            return unsafe { wasm::eq_ignore_ascii_case_simd128(a, b) };
        }
    }
    eq_ignore_ascii_case_scalar(a, b)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_slices() {
        assert!(eq_ignore_ascii_case(b"", b""));
        assert!(eq_ignore_ascii_case_scalar(b"", b""));
    }

    #[test]
    fn different_lengths() {
        assert!(!eq_ignore_ascii_case(b"abc", b"ab"));
        assert!(!eq_ignore_ascii_case(b"ab", b"abc"));
        assert!(!eq_ignore_ascii_case_scalar(b"abc", b"ab"));
    }

    #[test]
    fn exact_match() {
        assert!(eq_ignore_ascii_case(b"hello", b"hello"));
        assert!(eq_ignore_ascii_case_scalar(b"hello", b"hello"));
    }

    #[test]
    fn case_differs() {
        assert!(eq_ignore_ascii_case(b"Hello", b"hELLO"));
        assert!(eq_ignore_ascii_case_scalar(b"Hello", b"hELLO"));
    }

    #[test]
    fn mismatch() {
        assert!(!eq_ignore_ascii_case(b"Hello", b"Hxllo"));
        assert!(!eq_ignore_ascii_case_scalar(b"Hello", b"Hxllo"));
    }

    #[test]
    fn non_ascii_exact() {
        let a: &[u8] = &[0x80, 0x90, 0xFF];
        let b: &[u8] = &[0x80, 0x90, 0xFF];
        assert!(eq_ignore_ascii_case(a, b));
        assert!(eq_ignore_ascii_case_scalar(a, b));

        // Non-ASCII bytes that differ must not match.
        let c: &[u8] = &[0x80, 0x91, 0xFF];
        assert!(!eq_ignore_ascii_case(a, c));
        assert!(!eq_ignore_ascii_case_scalar(a, c));
    }

    #[test]
    fn long_match() {
        // >32 bytes with mixed case
        let a = b"The Quick Brown Fox Jumps Over The Lazy Dog!";
        let b = b"tHE qUICK bROWN fOX jUMPS oVER tHE lAZY dOG!";
        assert_eq!(a.len(), b.len());
        assert!(a.len() > 32);
        assert!(eq_ignore_ascii_case(a, b));
        assert!(eq_ignore_ascii_case_scalar(a, b));
    }

    #[test]
    fn exactly_16_bytes() {
        let a = b"ABCDefghIJKLmnop";
        let b = b"abcdEFGHijklMNOP";
        assert_eq!(a.len(), 16);
        assert!(eq_ignore_ascii_case(a, b));
        assert!(eq_ignore_ascii_case_scalar(a, b));
    }

    #[test]
    fn exactly_32_bytes() {
        let a = b"ABCDefghIJKLmnopQRSTuvwxYZ012345";
        let b = b"abcdEFGHijklMNOPqrstUVWXyz012345";
        assert_eq!(a.len(), 32);
        assert!(eq_ignore_ascii_case(a, b));
        assert!(eq_ignore_ascii_case_scalar(a, b));
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

            // Generate ASCII bytes.
            let a: Vec<u8> = (0..len)
                .map(|_| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    (rng % 128) as u8
                })
                .collect();

            // Copy and randomly toggle case on ASCII letters.
            let b: Vec<u8> = a
                .iter()
                .map(|&byte| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    if byte.is_ascii_alphabetic() && (rng & 1) == 0 {
                        byte ^ 0x20 // toggle case
                    } else {
                        byte
                    }
                })
                .collect();

            let expected = eq_ignore_ascii_case_scalar(&a, &b);
            let got = eq_ignore_ascii_case(&a, &b);
            assert_eq!(
                got, expected,
                "mismatch for len={len}, a={a:?}, b={b:?}"
            );
        }
    }
}
