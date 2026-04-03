//! Whitespace run skipping with SIMD acceleration.
//!
//! Provides [`skip_whitespace`], which returns the count of leading XML
//! whitespace bytes (0x20 space, 0x09 tab, 0x0A newline, 0x0D carriage return).
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

    /// AVX2 implementation -- processes 32 bytes at a time.
    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn skip_whitespace_avx2(data: &[u8]) -> usize {
        let len = data.len();
        let mut i: usize = 0;

        // SAFETY: AVX2 is enabled by `target_feature`.
        unsafe {
            let ws_space = _mm256_set1_epi8(0x20_u8 as i8);
            let ws_tab = _mm256_set1_epi8(0x09_u8 as i8);
            let ws_nl = _mm256_set1_epi8(0x0A_u8 as i8);
            let ws_cr = _mm256_set1_epi8(0x0D_u8 as i8);

            // --- 32-byte AVX2 chunks ---
            while i + 32 <= len {
                // SAFETY: `i + 32 <= len <= data.len()`, so the 32-byte
                // unaligned load is within bounds.
                let chunk = _mm256_loadu_si256(data.as_ptr().add(i) as *const __m256i);
                let combined = _mm256_or_si256(
                    _mm256_or_si256(
                        _mm256_cmpeq_epi8(chunk, ws_space),
                        _mm256_cmpeq_epi8(chunk, ws_tab),
                    ),
                    _mm256_or_si256(
                        _mm256_cmpeq_epi8(chunk, ws_nl),
                        _mm256_cmpeq_epi8(chunk, ws_cr),
                    ),
                );
                // Bit N = 1 if byte N is whitespace.
                let mask = _mm256_movemask_epi8(combined) as u32;
                if mask != 0xFFFF_FFFF {
                    // First non-whitespace byte: first 0-bit position.
                    return i + (!mask).trailing_zeros() as usize;
                }
                i += 32;
            }

            // --- 16-byte SSE2 tail ---
            if i + 16 <= len {
                // SAFETY: `i + 16 <= len`. SSE2 is implied by AVX2.
                let chunk = _mm_loadu_si128(data.as_ptr().add(i) as *const __m128i);
                let ws_space_128 = _mm_set1_epi8(0x20_u8 as i8);
                let ws_tab_128 = _mm_set1_epi8(0x09_u8 as i8);
                let ws_nl_128 = _mm_set1_epi8(0x0A_u8 as i8);
                let ws_cr_128 = _mm_set1_epi8(0x0D_u8 as i8);
                let combined = _mm_or_si128(
                    _mm_or_si128(
                        _mm_cmpeq_epi8(chunk, ws_space_128),
                        _mm_cmpeq_epi8(chunk, ws_tab_128),
                    ),
                    _mm_or_si128(
                        _mm_cmpeq_epi8(chunk, ws_nl_128),
                        _mm_cmpeq_epi8(chunk, ws_cr_128),
                    ),
                );
                let mask = _mm_movemask_epi8(combined) as u32;
                if mask != 0xFFFF {
                    return i + (!mask).trailing_zeros() as usize;
                }
                i += 16;
            }
        }

        // --- scalar tail ---
        while i < len {
            match data[i] {
                b' ' | b'\t' | b'\n' | b'\r' => i += 1,
                _ => return i,
            }
        }
        i
    }

    /// SSE2 implementation -- processes 16 bytes at a time.
    #[target_feature(enable = "sse2")]
    pub(super) unsafe fn skip_whitespace_sse2(data: &[u8]) -> usize {
        let len = data.len();
        let mut i: usize = 0;

        // SAFETY: SSE2 is enabled by `target_feature`.
        unsafe {
            let ws_space = _mm_set1_epi8(0x20_u8 as i8);
            let ws_tab = _mm_set1_epi8(0x09_u8 as i8);
            let ws_nl = _mm_set1_epi8(0x0A_u8 as i8);
            let ws_cr = _mm_set1_epi8(0x0D_u8 as i8);

            // --- 16-byte SSE2 chunks ---
            while i + 16 <= len {
                // SAFETY: `i + 16 <= len <= data.len()`. SSE2 is enabled by
                // `target_feature`.
                let chunk = _mm_loadu_si128(data.as_ptr().add(i) as *const __m128i);
                let combined = _mm_or_si128(
                    _mm_or_si128(
                        _mm_cmpeq_epi8(chunk, ws_space),
                        _mm_cmpeq_epi8(chunk, ws_tab),
                    ),
                    _mm_or_si128(
                        _mm_cmpeq_epi8(chunk, ws_nl),
                        _mm_cmpeq_epi8(chunk, ws_cr),
                    ),
                );
                let mask = _mm_movemask_epi8(combined) as u32;
                if mask != 0xFFFF {
                    return i + (!mask).trailing_zeros() as usize;
                }
                i += 16;
            }
        }

        // --- scalar tail ---
        while i < len {
            match data[i] {
                b' ' | b'\t' | b'\n' | b'\r' => i += 1,
                _ => return i,
            }
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

    /// NEON implementation -- processes 16 bytes at a time.
    pub(super) unsafe fn skip_whitespace_neon(data: &[u8]) -> usize {
        let len = data.len();
        let mut i: usize = 0;

        // SAFETY: NEON is always available on aarch64.
        unsafe {
            let ws_space = vdupq_n_u8(0x20);
            let ws_tab = vdupq_n_u8(0x09);
            let ws_nl = vdupq_n_u8(0x0A);
            let ws_cr = vdupq_n_u8(0x0D);

            while i + 16 <= len {
                // SAFETY: `i + 16 <= len <= data.len()`.
                let chunk = vld1q_u8(data.as_ptr().add(i));
                let combined = vorrq_u8(
                    vorrq_u8(
                        vceqq_u8(chunk, ws_space),
                        vceqq_u8(chunk, ws_tab),
                    ),
                    vorrq_u8(
                        vceqq_u8(chunk, ws_nl),
                        vceqq_u8(chunk, ws_cr),
                    ),
                );

                // Fast check: if all lanes are 0xFF, all 16 bytes are whitespace.
                if vminvq_u8(combined) == 0xFF {
                    i += 16;
                    continue;
                }

                // Find first non-whitespace: invert then find first non-zero.
                let neq = vmvnq_u8(combined);
                let as_u64 = vreinterpretq_u64_u8(neq);
                let lo = vgetq_lane_u64::<0>(as_u64);
                if lo != 0 {
                    return i + (lo.trailing_zeros() / 8) as usize;
                }
                let hi = vgetq_lane_u64::<1>(as_u64);
                return i + 8 + (hi.trailing_zeros() / 8) as usize;
            }
        }

        // --- scalar tail ---
        while i < len {
            match data[i] {
                b' ' | b'\t' | b'\n' | b'\r' => i += 1,
                _ => return i,
            }
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

    /// SIMD128 implementation -- processes 16 bytes at a time.
    #[allow(dead_code)]
    #[target_feature(enable = "simd128")]
    pub(super) unsafe fn skip_whitespace_simd128(data: &[u8]) -> usize {
        let len = data.len();
        let mut i: usize = 0;

        // SAFETY: simd128 is enabled by `target_feature`.
        unsafe {
            let ws_space = u8x16_splat(0x20);
            let ws_tab = u8x16_splat(0x09);
            let ws_nl = u8x16_splat(0x0A);
            let ws_cr = u8x16_splat(0x0D);

            while i + 16 <= len {
                // SAFETY: `i + 16 <= len <= data.len()`.
                let chunk = v128_load(data.as_ptr().add(i) as *const v128);
                let combined = v128_or(
                    v128_or(
                        i8x16_eq(chunk, ws_space),
                        i8x16_eq(chunk, ws_tab),
                    ),
                    v128_or(
                        i8x16_eq(chunk, ws_nl),
                        i8x16_eq(chunk, ws_cr),
                    ),
                );
                // Bit N = 1 if byte N is whitespace.
                let mask = i8x16_bitmask(combined) as u32;
                if mask != 0xFFFF {
                    return i + (!mask).trailing_zeros() as usize;
                }
                i += 16;
            }
        }

        // --- scalar tail ---
        while i < len {
            match data[i] {
                b' ' | b'\t' | b'\n' | b'\r' => i += 1,
                _ => return i,
            }
        }
        i
    }
}

// ---------------------------------------------------------------------------
// Scalar fallback
// ---------------------------------------------------------------------------

/// Byte-by-byte whitespace skip (portable fallback).
pub(crate) fn skip_whitespace_scalar(data: &[u8]) -> usize {
    data.iter()
        .take_while(|&&b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
        .count()
}

// ---------------------------------------------------------------------------
// Dispatch function
// ---------------------------------------------------------------------------

/// Returns the number of leading XML whitespace bytes (0x20, 0x09, 0x0A, 0x0D)
/// in `data`. Returns 0 if `data` is empty or the first byte is not whitespace.
/// Returns `data.len()` if all bytes are whitespace.
///
/// Selects the best available SIMD implementation at runtime.
#[allow(unreachable_code)]
pub(crate) fn skip_whitespace(data: &[u8]) -> usize {
    if data.is_empty() {
        return 0;
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: AVX2 feature is confirmed present by the runtime check.
            return unsafe { x86::skip_whitespace_avx2(data) };
        }
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: SSE2 is always available on x86_64.
            return unsafe { x86::skip_whitespace_sse2(data) };
        }
        #[cfg(target_arch = "x86")]
        if is_x86_feature_detected!("sse2") {
            // SAFETY: SSE2 feature is confirmed present by the runtime check.
            return unsafe { x86::skip_whitespace_sse2(data) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64.
        return unsafe { aarch64::skip_whitespace_neon(data) };
    }
    #[cfg(target_arch = "wasm32")]
    {
        #[cfg(target_feature = "simd128")]
        {
            // SAFETY: simd128 target feature is statically enabled.
            return unsafe { wasm::skip_whitespace_simd128(data) };
        }
    }
    skip_whitespace_scalar(data)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        assert_eq!(skip_whitespace(b""), 0);
        assert_eq!(skip_whitespace_scalar(b""), 0);
    }

    #[test]
    fn no_whitespace() {
        assert_eq!(skip_whitespace(b"hello"), 0);
        assert_eq!(skip_whitespace_scalar(b"hello"), 0);
    }

    #[test]
    fn all_spaces() {
        let data = vec![b' '; 64];
        assert_eq!(skip_whitespace(&data), 64);
        assert_eq!(skip_whitespace_scalar(&data), 64);
    }

    #[test]
    fn all_tabs() {
        let data = vec![b'\t'; 64];
        assert_eq!(skip_whitespace(&data), 64);
    }

    #[test]
    fn all_newlines() {
        let data = vec![b'\n'; 64];
        assert_eq!(skip_whitespace(&data), 64);
    }

    #[test]
    fn all_carriage_returns() {
        let data = vec![b'\r'; 64];
        assert_eq!(skip_whitespace(&data), 64);
    }

    #[test]
    fn mixed_whitespace() {
        assert_eq!(skip_whitespace(b" \t\n\r hello"), 5);
        assert_eq!(skip_whitespace_scalar(b" \t\n\r hello"), 5);
    }

    #[test]
    fn single_space() {
        assert_eq!(skip_whitespace(b" x"), 1);
    }

    #[test]
    fn exactly_16_ws() {
        let mut data = vec![b' '; 16];
        data.push(b'x');
        assert_eq!(skip_whitespace(&data), 16);
    }

    #[test]
    fn exactly_32_ws() {
        let mut data = vec![b' '; 32];
        data.push(b'x');
        assert_eq!(skip_whitespace(&data), 32);
    }

    #[test]
    fn ws_at_boundary_48() {
        let mut data = vec![b'\t'; 48];
        data.push(b'a');
        assert_eq!(skip_whitespace(&data), 48);
    }

    #[test]
    fn non_xml_whitespace_not_skipped() {
        // 0x0C (form feed) is NOT in the XML whitespace set.
        assert_eq!(skip_whitespace(&[0x0C, b' ']), 0);
        // 0x0B (vertical tab) is NOT in the XML whitespace set.
        assert_eq!(skip_whitespace(&[0x0B, b' ']), 0);
    }

    #[test]
    fn property_simd_matches_scalar() {
        let mut rng: u64 = 0xDEAD_BEEF_CAFE_BABE;
        let ws_bytes: [u8; 4] = [0x20, 0x09, 0x0A, 0x0D];

        for _ in 0..2000 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;

            let ws_len = (rng % 200) as usize;
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let tail_len = (rng % 50) as usize;

            // Generate whitespace prefix.
            let mut data: Vec<u8> = (0..ws_len)
                .map(|_| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    ws_bytes[(rng % 4) as usize]
                })
                .collect();

            // Append non-whitespace tail.
            for _ in 0..tail_len {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                // Pick a byte that is NOT whitespace.
                let b = loop {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    let candidate = (rng & 0xFF) as u8;
                    if !matches!(candidate, b' ' | b'\t' | b'\n' | b'\r') {
                        break candidate;
                    }
                };
                data.push(b);
            }

            let expected = skip_whitespace_scalar(&data);
            let got = skip_whitespace(&data);
            assert_eq!(
                got, expected,
                "mismatch for ws_len={ws_len}, tail_len={tail_len}"
            );
        }
    }
}
