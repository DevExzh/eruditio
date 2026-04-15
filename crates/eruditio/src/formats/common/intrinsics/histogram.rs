//! Byte frequency counting with multi-array optimization.
//!
//! Uses 4 independent counter arrays to avoid store-forwarding stalls,
//! a micro-architectural optimization that reduces memory dependency
//! bottlenecks on large inputs. The final merge of the 4 histogram
//! arrays is SIMD-vectorized (AVX2/SSE2/NEON) for throughput.

use super::prefetch::prefetch_read_l1;

/// Counts the frequency of each byte value in `data` using a single counter array.
#[allow(dead_code)]
pub(crate) fn byte_histogram_scalar(data: &[u8]) -> [u32; 256] {
    let mut counts = [0u32; 256];
    for &b in data {
        counts[b as usize] += 1;
    }
    counts
}

// ---------------------------------------------------------------------------
// x86_64 SIMD merge implementations
// ---------------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
mod x86 {
    use core::arch::x86_64::*;

    /// AVX2 merge: processes 8 u32s (256 bits) per iteration, 32 iterations total.
    ///
    /// Prefetches the next cache line of c1/c2/c3 ahead of the current iteration
    /// to keep the L1 data cache warm.
    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn merge_histograms_avx2(
        c0: &mut [u32; 256],
        c1: &[u32; 256],
        c2: &[u32; 256],
        c3: &[u32; 256],
    ) {
        let c0_ptr = c0.as_mut_ptr();
        let c1_ptr = c1.as_ptr();
        let c2_ptr = c2.as_ptr();
        let c3_ptr = c3.as_ptr();

        // 256 u32s / 8 per AVX2 register = 32 iterations.
        // Each iteration processes 32 bytes (8 * 4 bytes per u32).
        // A cache line is 64 bytes = 16 u32s = 2 iterations.
        unsafe {
            for i in 0..32 {
                let offset = i * 8;

                // Prefetch next cache line of c1/c2/c3 (64 bytes = 16 u32s ahead).
                if i % 2 == 0 && i + 2 < 32 {
                    let prefetch_offset = (i + 2) * 8;
                    super::prefetch_read_l1(c1_ptr.add(prefetch_offset) as *const u8);
                    super::prefetch_read_l1(c2_ptr.add(prefetch_offset) as *const u8);
                    super::prefetch_read_l1(c3_ptr.add(prefetch_offset) as *const u8);
                }

                let v0 = _mm256_loadu_si256(c0_ptr.add(offset) as *const __m256i);
                let v1 = _mm256_loadu_si256(c1_ptr.add(offset) as *const __m256i);
                let v2 = _mm256_loadu_si256(c2_ptr.add(offset) as *const __m256i);
                let v3 = _mm256_loadu_si256(c3_ptr.add(offset) as *const __m256i);

                let sum01 = _mm256_add_epi32(v0, v1);
                let sum23 = _mm256_add_epi32(v2, v3);
                let total = _mm256_add_epi32(sum01, sum23);

                _mm256_storeu_si256(c0_ptr.add(offset) as *mut __m256i, total);
            }
        }
    }

    /// SSE2 merge: processes 4 u32s (128 bits) per iteration, 64 iterations total.
    #[target_feature(enable = "sse2")]
    pub(super) unsafe fn merge_histograms_sse2(
        c0: &mut [u32; 256],
        c1: &[u32; 256],
        c2: &[u32; 256],
        c3: &[u32; 256],
    ) {
        let c0_ptr = c0.as_mut_ptr();
        let c1_ptr = c1.as_ptr();
        let c2_ptr = c2.as_ptr();
        let c3_ptr = c3.as_ptr();

        // 256 u32s / 4 per SSE2 register = 64 iterations.
        unsafe {
            for i in 0..64 {
                let offset = i * 4;

                let v0 = _mm_loadu_si128(c0_ptr.add(offset) as *const __m128i);
                let v1 = _mm_loadu_si128(c1_ptr.add(offset) as *const __m128i);
                let v2 = _mm_loadu_si128(c2_ptr.add(offset) as *const __m128i);
                let v3 = _mm_loadu_si128(c3_ptr.add(offset) as *const __m128i);

                let sum01 = _mm_add_epi32(v0, v1);
                let sum23 = _mm_add_epi32(v2, v3);
                let total = _mm_add_epi32(sum01, sum23);

                _mm_storeu_si128(c0_ptr.add(offset) as *mut __m128i, total);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// aarch64 NEON merge implementation
// ---------------------------------------------------------------------------

#[cfg(target_arch = "aarch64")]
mod aarch64 {
    use core::arch::aarch64::*;

    /// NEON merge: processes 4 u32s (128 bits) per iteration, 64 iterations total.
    pub(super) unsafe fn merge_histograms_neon(
        c0: &mut [u32; 256],
        c1: &[u32; 256],
        c2: &[u32; 256],
        c3: &[u32; 256],
    ) {
        let c0_ptr = c0.as_mut_ptr();
        let c1_ptr = c1.as_ptr();
        let c2_ptr = c2.as_ptr();
        let c3_ptr = c3.as_ptr();

        // 256 u32s / 4 per NEON register = 64 iterations.
        unsafe {
            for i in 0..64 {
                let offset = i * 4;

                let v0 = vld1q_u32(c0_ptr.add(offset));
                let v1 = vld1q_u32(c1_ptr.add(offset));
                let v2 = vld1q_u32(c2_ptr.add(offset));
                let v3 = vld1q_u32(c3_ptr.add(offset));

                let sum01 = vaddq_u32(v0, v1);
                let sum23 = vaddq_u32(v2, v3);
                let total = vaddq_u32(sum01, sum23);

                vst1q_u32(c0_ptr.add(offset), total);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Scalar merge fallback
// ---------------------------------------------------------------------------

/// Scalar merge: adds c1, c2, c3 into c0 element-by-element.
#[inline]
fn merge_histograms_scalar(c0: &mut [u32; 256], c1: &[u32; 256], c2: &[u32; 256], c3: &[u32; 256]) {
    for i in 0..256 {
        c0[i] += c1[i] + c2[i] + c3[i];
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Merges four histogram arrays by adding c1, c2, c3 into c0.
///
/// Selects the best available SIMD implementation at runtime:
/// - AVX2: 8 u32s per iteration (32 iterations)
/// - SSE2: 4 u32s per iteration (64 iterations)
/// - NEON: 4 u32s per iteration (64 iterations)
/// - Scalar fallback for other architectures
#[allow(unreachable_code)]
#[inline]
fn merge_histograms(c0: &mut [u32; 256], c1: &[u32; 256], c2: &[u32; 256], c3: &[u32; 256]) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: AVX2 feature is confirmed present by the runtime check.
            unsafe {
                x86::merge_histograms_avx2(c0, c1, c2, c3);
            }
            return;
        }
        // SAFETY: SSE2 is always available on x86_64.
        unsafe {
            x86::merge_histograms_sse2(c0, c1, c2, c3);
        }
        return;
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: NEON is always available on aarch64.
        unsafe {
            aarch64::merge_histograms_neon(c0, c1, c2, c3);
        }
        return;
    }
    merge_histograms_scalar(c0, c1, c2, c3);
}

/// Counts the frequency of each byte value in `data`.
///
/// Uses 4 independent counter arrays to avoid store-forwarding stalls
/// when consecutive bytes index the same cache line. The final merge
/// of the 4 arrays is SIMD-vectorized for throughput.
#[allow(dead_code)]
#[inline]
pub(crate) fn byte_histogram(data: &[u8]) -> [u32; 256] {
    if data.len() < 64 {
        return byte_histogram_scalar(data);
    }

    let mut c0 = [0u32; 256];
    let mut c1 = [0u32; 256];
    let mut c2 = [0u32; 256];
    let mut c3 = [0u32; 256];

    // Process 4 bytes per iteration, each into a different counter array.
    let chunks = data.len() / 4;
    let remainder = data.len() % 4;

    for i in 0..chunks {
        let base = i * 4;
        c0[data[base] as usize] += 1;
        c1[data[base + 1] as usize] += 1;
        c2[data[base + 2] as usize] += 1;
        c3[data[base + 3] as usize] += 1;
    }

    // Handle remaining bytes.
    let tail_start = chunks * 4;
    for i in 0..remainder {
        c0[data[tail_start + i] as usize] += 1;
    }

    // Merge the 4 arrays using SIMD-vectorized addition.
    merge_histograms(&mut c0, &c1, &c2, &c3);

    c0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        let counts = byte_histogram(&[]);
        assert!(counts.iter().all(|&c| c == 0));
        assert_eq!(counts.iter().copied().sum::<u32>(), 0);
    }

    #[test]
    fn single_byte() {
        let counts = byte_histogram(b"A");
        assert_eq!(counts[b'A' as usize], 1);
        assert_eq!(counts.iter().copied().sum::<u32>(), 1);
    }

    #[test]
    fn all_same_byte() {
        let data = vec![0x42u8; 1000];
        let counts = byte_histogram(&data);
        assert_eq!(counts[0x42], 1000);
        assert_eq!(counts.iter().copied().sum::<u32>(), 1000);
    }

    #[test]
    fn all_256_values() {
        let data: Vec<u8> = (0u8..=255).collect();
        let counts = byte_histogram(&data);
        assert!(counts.iter().all(|&c| c == 1));
    }

    #[test]
    fn known_distribution() {
        let data = b"aaabbc";
        let counts = byte_histogram(data);
        assert_eq!(counts[b'a' as usize], 3);
        assert_eq!(counts[b'b' as usize], 2);
        assert_eq!(counts[b'c' as usize], 1);
        assert_eq!(counts.iter().copied().sum::<u32>(), 6);
    }

    #[test]
    fn matches_scalar_for_large_input() {
        // Build a deterministic pseudo-random input of 10000 bytes.
        let mut data = vec![0u8; 10_000];
        let mut state: u32 = 0xDEAD_BEEF;
        for b in data.iter_mut() {
            // xorshift32
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *b = state as u8;
        }

        let scalar = byte_histogram_scalar(&data);
        let multi = byte_histogram(&data);
        assert_eq!(scalar, multi);
    }

    #[test]
    fn property_multi_array_matches_scalar() {
        for seed in 0u32..500 {
            let mut state = seed.wrapping_add(1);
            // Derive a length from the seed (0..5000).
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let len = (state % 5001) as usize;

            let mut data = vec![0u8; len];
            for b in data.iter_mut() {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *b = state as u8;
            }

            let scalar = byte_histogram_scalar(&data);
            let multi = byte_histogram(&data);
            assert_eq!(scalar, multi, "mismatch at seed={seed}, len={len}");
        }
    }
}
