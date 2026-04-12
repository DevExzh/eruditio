//! Bulk ASCII validation with SIMD acceleration.
//!
//! Provides [`is_all_ascii`], which returns `true` if every byte in the input
//! is in the ASCII range (0x00-0x7F).
//!
//! The dispatch function selects the best available SIMD backend at runtime:
//!
//! | Architecture  | Backend                                        |
//! |---------------|------------------------------------------------|
//! | x86 / x86_64  | AVX512BW (64 B) then AVX2 (32 B) then SSE2 fallback |
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

    /// AVX512BW implementation -- processes 64 bytes at a time.
    ///
    /// Uses an OR-accumulator with 512-bit zmm registers, doubling throughput
    /// over AVX2.  The `vpmovb2m` instruction extracts high bits directly into
    /// a 64-bit mask register, avoiding the movemask bottleneck.
    #[target_feature(enable = "avx512bw")]
    pub(super) unsafe fn is_all_ascii_avx512bw(data: &[u8]) -> bool {
        let len = data.len();
        let mut i: usize = 0;

        // --- 64-byte AVX512BW chunks with OR-accumulator ---
        unsafe {
            let mut acc = _mm512_setzero_si512();
            while i + 64 <= len {
                let chunk = _mm512_loadu_si512(data.as_ptr().add(i) as *const __m512i);
                acc = _mm512_or_si512(acc, chunk);
                i += 64;
            }
            // _mm512_movepi8_mask extracts bit 7 from each byte → __mmask64.
            if _mm512_movepi8_mask(acc) != 0 {
                return false;
            }
        }

        // --- 32-byte AVX2 tail ---
        if i + 32 <= len {
            unsafe {
                let chunk = _mm256_loadu_si256(data.as_ptr().add(i) as *const __m256i);
                if _mm256_movemask_epi8(chunk) as u32 != 0 {
                    return false;
                }
            }
            i += 32;
        }

        // --- 16-byte SSE2 tail ---
        if i + 16 <= len {
            unsafe {
                let chunk = _mm_loadu_si128(data.as_ptr().add(i) as *const __m128i);
                if _mm_movemask_epi8(chunk) as u32 != 0 {
                    return false;
                }
            }
            i += 16;
        }

        // --- overlapping SSE2 tail (branchless) ---
        if i < len {
            if len >= 16 {
                unsafe {
                    let chunk = _mm_loadu_si128(data.as_ptr().add(len - 16) as *const __m128i);
                    if _mm_movemask_epi8(chunk) as u32 != 0 {
                        return false;
                    }
                }
            } else {
                let ptr = data.as_ptr();
                let mut acc: u64;
                if len >= 8 {
                    unsafe {
                        acc = (ptr as *const u64).read_unaligned();
                        acc |= (ptr.add(len - 8) as *const u64).read_unaligned();
                    }
                } else if len >= 4 {
                    unsafe {
                        acc = (ptr as *const u32).read_unaligned() as u64;
                        acc |= (ptr.add(len - 4) as *const u32).read_unaligned() as u64;
                    }
                } else if len >= 2 {
                    unsafe {
                        acc = (ptr as *const u16).read_unaligned() as u64;
                        acc |= (ptr.add(len - 2) as *const u16).read_unaligned() as u64;
                    }
                } else {
                    acc = data[0] as u64;
                }
                if acc & 0x8080_8080_8080_8080 != 0 {
                    return false;
                }
            }
        }
        true
    }

    /// AVX2 implementation -- processes 32 bytes at a time.
    ///
    /// Uses an OR-accumulator to defer the high-bit check until after the
    /// main loop, reducing the loop body to load + OR (no branch per chunk).
    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn is_all_ascii_avx2(data: &[u8]) -> bool {
        let len = data.len();
        let mut i: usize = 0;

        // --- 32-byte AVX2 chunks with OR-accumulator ---
        // SAFETY: AVX2 is enabled by `target_feature`.
        unsafe {
            let mut acc = _mm256_setzero_si256();
            while i + 32 <= len {
                // SAFETY: `i + 32 <= len <= data.len()`, so the 32-byte
                // unaligned load is within bounds.
                let chunk = _mm256_loadu_si256(data.as_ptr().add(i) as *const __m256i);
                acc = _mm256_or_si256(acc, chunk);
                i += 32;
            }
            // Check accumulated high bits: any non-ASCII byte in any chunk
            // will have set bit 7 in the corresponding accumulator lane.
            if _mm256_movemask_epi8(acc) as u32 != 0 {
                return false;
            }
        }

        // --- 16-byte SSE2 tail ---
        if i + 16 <= len {
            // SAFETY: `i + 16 <= len`. SSE2 is implied by AVX2.
            unsafe {
                let chunk = _mm_loadu_si128(data.as_ptr().add(i) as *const __m128i);
                let mask = _mm_movemask_epi8(chunk) as u32;
                if mask != 0 {
                    return false;
                }
            }
            i += 16;
        }

        // --- overlapping SSE2 tail (branchless) ---
        // Instead of a per-byte scalar loop for the remaining 0-15 bytes,
        // re-check the last 16 bytes with a single SSE2 load. The overlap
        // with already-verified bytes is harmless. This eliminates up to 15
        // branch mispredictions per call.
        if i < len {
            if len >= 16 {
                unsafe {
                    let chunk = _mm_loadu_si128(data.as_ptr().add(len - 16) as *const __m128i);
                    let mask = _mm_movemask_epi8(chunk) as u32;
                    if mask != 0 {
                        return false;
                    }
                }
            } else {
                // Input shorter than 16 bytes total: use widened word loads
                // to avoid a per-byte loop whose exit branch is hard to predict
                // (variable trip count 1-15 causes ~18% misprediction rate).
                // Overlapping reads with u64/u32/u16 cover all bytes in at most
                // 4 non-looping branches.
                let ptr = data.as_ptr();
                let mut acc: u64;
                if len >= 8 {
                    // Two overlapping u64 reads cover bytes 0..len.
                    unsafe {
                        acc = (ptr as *const u64).read_unaligned();
                        acc |= (ptr.add(len - 8) as *const u64).read_unaligned();
                    }
                } else if len >= 4 {
                    unsafe {
                        acc = (ptr as *const u32).read_unaligned() as u64;
                        acc |= (ptr.add(len - 4) as *const u32).read_unaligned() as u64;
                    }
                } else if len >= 2 {
                    unsafe {
                        acc = (ptr as *const u16).read_unaligned() as u64;
                        acc |= (ptr.add(len - 2) as *const u16).read_unaligned() as u64;
                    }
                } else {
                    // Exactly 1 byte.
                    acc = data[0] as u64;
                }
                if acc & 0x8080_8080_8080_8080 != 0 {
                    return false;
                }
            }
        }
        true
    }

    /// SSE2 implementation -- processes 16 bytes at a time.
    ///
    /// Uses an OR-accumulator to defer the high-bit check until after the
    /// main loop, reducing the loop body to load + OR (no branch per chunk).
    #[target_feature(enable = "sse2")]
    pub(super) unsafe fn is_all_ascii_sse2(data: &[u8]) -> bool {
        let len = data.len();
        let mut i: usize = 0;

        // --- 16-byte SSE2 chunks with OR-accumulator ---
        // SAFETY: SSE2 is enabled by `target_feature`.
        unsafe {
            let mut acc = _mm_setzero_si128();
            while i + 16 <= len {
                // SAFETY: `i + 16 <= len <= data.len()`, so the 16-byte
                // unaligned load is within bounds.
                let chunk = _mm_loadu_si128(data.as_ptr().add(i) as *const __m128i);
                acc = _mm_or_si128(acc, chunk);
                i += 16;
            }
            // Check accumulated high bits: any non-ASCII byte in any chunk
            // will have set bit 7 in the corresponding accumulator lane.
            if _mm_movemask_epi8(acc) as u32 != 0 {
                return false;
            }
        }

        // --- overlapping SSE2 tail ---
        if i < len && len >= 16 {
            unsafe {
                let chunk = _mm_loadu_si128(data.as_ptr().add(len - 16) as *const __m128i);
                let mask = _mm_movemask_epi8(chunk) as u32;
                if mask != 0 {
                    return false;
                }
            }
        } else {
            while i < len {
                if data[i] >= 0x80 {
                    return false;
                }
                i += 1;
            }
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

    /// NEON implementation -- processes 16 bytes at a time.
    ///
    /// Uses an OR-accumulator to defer the high-bit check until after the
    /// main loop, reducing the loop body to load + OR (no branch per chunk).
    pub(super) unsafe fn is_all_ascii_neon(data: &[u8]) -> bool {
        let len = data.len();
        let mut i: usize = 0;

        // --- 16-byte NEON chunks with OR-accumulator ---
        // SAFETY: NEON is always available on aarch64.
        unsafe {
            let mut acc = vdupq_n_u8(0);
            while i + 16 <= len {
                // SAFETY: `i + 16 <= len <= data.len()`, so the 16-byte
                // load is within bounds.
                let chunk = vld1q_u8(data.as_ptr().add(i));
                acc = vorrq_u8(acc, chunk);
                i += 16;
            }
            // Check accumulated high bits: if the maximum byte value >= 0x80,
            // at least one byte across all chunks was non-ASCII.
            if vmaxvq_u8(acc) >= 0x80 {
                return false;
            }
        }

        // --- scalar tail ---
        while i < len {
            if data[i] >= 0x80 {
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

    /// SIMD128 implementation -- processes 16 bytes at a time.
    ///
    /// Uses an OR-accumulator to defer the high-bit check until after the
    /// main loop, reducing the loop body to load + OR (no branch per chunk).
    #[allow(dead_code)]
    #[target_feature(enable = "simd128")]
    pub(super) unsafe fn is_all_ascii_simd128(data: &[u8]) -> bool {
        let len = data.len();
        let mut i: usize = 0;

        // --- 16-byte SIMD128 chunks with OR-accumulator ---
        // SAFETY: simd128 is enabled by `target_feature`.
        unsafe {
            let mut acc = i8x16_splat(0);
            while i + 16 <= len {
                // SAFETY: `i + 16 <= len <= data.len()`, so the 16-byte
                // load is within bounds.
                let chunk = v128_load(data.as_ptr().add(i) as *const v128);
                acc = v128_or(acc, chunk);
                i += 16;
            }
            // Check accumulated high bits: i8x16_bitmask extracts bit 7
            // of each byte. Any non-zero result means non-ASCII was found.
            if i8x16_bitmask(acc) as u32 != 0 {
                return false;
            }
        }

        // --- scalar tail ---
        while i < len {
            if data[i] >= 0x80 {
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

/// Byte-by-byte ASCII check (portable fallback).
pub(crate) fn is_all_ascii_scalar(data: &[u8]) -> bool {
    data.iter().all(|&b| b < 0x80)
}

// ---------------------------------------------------------------------------
// Dispatch function
// ---------------------------------------------------------------------------

/// Returns `true` if every byte in `data` is in the ASCII range (0x00-0x7F).
/// Returns `true` for empty input.
///
/// Selects the best available SIMD implementation at runtime.
#[allow(unreachable_code)]
#[inline]
pub(crate) fn is_all_ascii(data: &[u8]) -> bool {
    if data.is_empty() {
        return true;
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if is_x86_feature_detected!("avx512bw") {
            // SAFETY: AVX512BW feature is confirmed present by the runtime check.
            return unsafe { x86::is_all_ascii_avx512bw(data) };
        }
        if is_x86_feature_detected!("avx2") {
            // SAFETY: AVX2 feature is confirmed present by the runtime check.
            return unsafe { x86::is_all_ascii_avx2(data) };
        }
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: SSE2 is always available on x86_64.
            return unsafe { x86::is_all_ascii_sse2(data) };
        }
        #[cfg(target_arch = "x86")]
        if is_x86_feature_detected!("sse2") {
            // SAFETY: SSE2 feature is confirmed present by the runtime check.
            return unsafe { x86::is_all_ascii_sse2(data) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64.
        return unsafe { aarch64::is_all_ascii_neon(data) };
    }
    #[cfg(target_arch = "wasm32")]
    {
        #[cfg(target_feature = "simd128")]
        {
            // SAFETY: simd128 target feature is statically enabled.
            return unsafe { wasm::is_all_ascii_simd128(data) };
        }
    }
    is_all_ascii_scalar(data)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        assert!(is_all_ascii(b""));
        assert!(is_all_ascii_scalar(b""));
    }

    #[test]
    fn pure_ascii() {
        assert!(is_all_ascii(b"Hello, World!"));
        assert!(is_all_ascii_scalar(b"Hello, World!"));
    }

    #[test]
    fn all_zero() {
        assert!(is_all_ascii(&[0u8; 64]));
    }

    #[test]
    fn all_0x7f() {
        assert!(is_all_ascii(&[0x7Fu8; 64]));
    }

    #[test]
    fn single_non_ascii_at_start() {
        assert!(!is_all_ascii(&[0x80]));
        assert!(!is_all_ascii_scalar(&[0x80]));
    }

    #[test]
    fn single_non_ascii_at_end() {
        let mut data = vec![b'A'; 33];
        data[32] = 0x80;
        assert!(!is_all_ascii(&data));
    }

    #[test]
    fn non_ascii_at_position_15() {
        let mut data = vec![b'A'; 32];
        data[15] = 0xFF;
        assert!(!is_all_ascii(&data));
    }

    #[test]
    fn non_ascii_at_position_16() {
        let mut data = vec![b'A'; 32];
        data[16] = 0xFF;
        assert!(!is_all_ascii(&data));
    }

    #[test]
    fn non_ascii_at_position_31() {
        let mut data = vec![b'A'; 32];
        data[31] = 0xFF;
        assert!(!is_all_ascii(&data));
    }

    #[test]
    fn all_0xff() {
        assert!(!is_all_ascii(&[0xFFu8; 64]));
    }

    #[test]
    fn exactly_16_bytes_ascii() {
        assert!(is_all_ascii(b"0123456789abcdef"));
    }

    #[test]
    fn exactly_32_bytes_ascii() {
        assert!(is_all_ascii(b"0123456789abcdef0123456789abcdef"));
    }

    #[test]
    fn short_input_under_16() {
        assert!(is_all_ascii(b"tiny"));
        assert!(!is_all_ascii(&[0x80]));
    }

    #[test]
    fn property_simd_matches_scalar() {
        let mut rng: u64 = 0xDEAD_BEEF_CAFE_BABE;

        for _ in 0..2000 {
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

            let expected = is_all_ascii_scalar(&data);
            let got = is_all_ascii(&data);
            assert_eq!(
                got,
                expected,
                "mismatch for len={len}, data[..min(8,len)]={:?}",
                &data[..len.min(8)]
            );
        }
    }
}
