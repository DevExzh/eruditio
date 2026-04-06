//! Byte frequency counting with multi-array optimization.
//!
//! Uses 4 independent counter arrays to avoid store-forwarding stalls,
//! a micro-architectural optimization that reduces memory dependency
//! bottlenecks on large inputs. This is not SIMD vectorization per se,
//! but achieves significant throughput improvement on all platforms.

/// Counts the frequency of each byte value in `data` using a single counter array.
#[allow(dead_code)]
pub(crate) fn byte_histogram_scalar(data: &[u8]) -> [u32; 256] {
    let mut counts = [0u32; 256];
    for &b in data {
        counts[b as usize] += 1;
    }
    counts
}

/// Counts the frequency of each byte value in `data`.
///
/// Uses 4 independent counter arrays to avoid store-forwarding stalls
/// when consecutive bytes index the same cache line.
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

    // Merge the 4 arrays.
    for i in 0..256 {
        c0[i] += c1[i] + c2[i] + c3[i];
    }

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
