//! PalmDoc LZ77 compression and decompression.
//!
//! Used by MOBI/PRC/PalmDOC text records. Each text record is independently
//! compressed/decompressed (max 4096 bytes uncompressed per record).
//!
//! Byte encoding scheme:
//! - `0x00`: literal null byte
//! - `0x01..=0x08`: copy next N bytes literally
//! - `0x09..=0x7F`: literal byte (self-representing)
//! - `0x80..=0xBF`: LZ77 back-reference (2-byte encoding)
//! - `0xC0..=0xFF`: space + (byte XOR 0x80)

use crate::error::{EruditioError, Result};

/// Maximum uncompressed text record size.
pub const RECORD_SIZE: usize = 4096;

/// Maximum decompression output size (256 MB) to prevent decompression bombs.
const MAX_DECOMPRESS_OUTPUT: usize = 256 * 1024 * 1024;

/// Maximum LZ77 back-reference distance.
const MAX_DISTANCE: usize = 2047;

/// Minimum LZ77 match length.
const MIN_MATCH: usize = 3;

/// Maximum LZ77 match length.
const MAX_MATCH: usize = 10;

// ---------------------------------------------------------------------------
// Hash-chain constants
// ---------------------------------------------------------------------------

/// Number of bits used for the hash table.
const HASH_BITS: usize = 12;

/// Size of the hash table (4096 entries).
const HASH_SIZE: usize = 1 << HASH_BITS;

/// Maximum number of chain links to follow per lookup.
const MAX_CHAIN: usize = 64;

/// Sentinel value indicating "no entry" in the hash chain.
const NO_ENTRY: u16 = 0xFFFF;

// ---------------------------------------------------------------------------
// Hash chain data structure
// ---------------------------------------------------------------------------

/// A hash-chain accelerator for LZ77 match finding, modelled after zlib.
///
/// `head[hash]` stores the most recent position with that hash value.
/// `prev[pos % RECORD_SIZE]` chains earlier positions with the same hash.
struct HashChain {
    head: [u16; HASH_SIZE],
    prev: [u16; RECORD_SIZE],
}

impl HashChain {
    /// Creates a new, empty hash chain.
    fn new() -> Self {
        Self {
            head: [NO_ENTRY; HASH_SIZE],
            prev: [NO_ENTRY; RECORD_SIZE],
        }
    }

    /// Computes a fast 12-bit hash of three consecutive bytes.
    #[inline]
    fn hash3(data: &[u8], pos: usize) -> usize {
        let h = ((data[pos] as u32) << 10) ^ ((data[pos + 1] as u32) << 5) ^ (data[pos + 2] as u32);
        (h as usize) & (HASH_SIZE - 1)
    }

    /// Inserts `pos` into the chain for `data[pos..pos+3]`.
    #[inline]
    fn insert(&mut self, data: &[u8], pos: usize) {
        if pos + 2 < data.len() {
            let h = Self::hash3(data, pos);
            self.prev[pos] = self.head[h];
            self.head[h] = pos as u16;
        }
    }

    /// Walks the chain for the hash of `data[pos..pos+3]` and returns the best
    /// `(distance, length)` pair, or `None` if no match of at least `MIN_MATCH`
    /// is found.
    fn find_best_match(&self, data: &[u8], pos: usize) -> Option<(usize, usize)> {
        let remaining = data.len() - pos;
        let max_len = remaining.min(MAX_MATCH);
        if max_len < MIN_MATCH {
            return None;
        }

        let h = Self::hash3(data, pos);
        let mut candidate = self.head[h];
        let mut best_distance: usize = 0;
        let mut best_length: usize = 0;
        let mut steps = 0;
        let window_start = pos.saturating_sub(MAX_DISTANCE);

        while candidate != NO_ENTRY && steps < MAX_CHAIN {
            let cand = candidate as usize;

            // Candidate must be before `pos` and inside the sliding window.
            if cand >= pos || cand < window_start {
                candidate = self.prev[cand];
                steps += 1;
                continue;
            }

            // SAFETY: match_length_simd requires that both slices have at
            // least `max_len` readable bytes.
            // For `data[pos..]`: `max_len <= remaining = data.len() - pos`,
            //   so `pos + max_len <= data.len()`.
            // For `data[cand..]`: `cand < pos`, so `data.len() - cand >
            //   data.len() - pos >= max_len`, giving `cand + max_len < data.len()`.
            let length = unsafe { match_length_simd(&data[cand..], &data[pos..], max_len) };

            if length >= MIN_MATCH && length > best_length {
                best_distance = pos - cand;
                best_length = length;
                if best_length == MAX_MATCH {
                    break; // Can't do better.
                }
            }

            candidate = self.prev[cand];
            steps += 1;
        }

        if best_length >= MIN_MATCH {
            Some((best_distance, best_length))
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// SIMD / scalar match-length comparison
// ---------------------------------------------------------------------------

/// Determines how many leading bytes of `a` and `b` are equal, up to
/// `max_len`. Uses SSE2 on x86_64 for the first 16 bytes; falls back to
/// scalar comparison otherwise.
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn match_length_simd(a: &[u8], b: &[u8], max_len: usize) -> usize {
    use std::arch::x86_64::*;

    // SAFETY: caller ensures both slices have at least `max_len` readable
    // bytes. We additionally check that each slice has at least 16 bytes
    // available so the 128-bit unaligned loads are valid.
    if max_len >= 16 && a.len() >= 16 && b.len() >= 16 {
        // SAFETY: length checks above guarantee 16 bytes are readable in
        // both `a` and `b`. SSE2 is available on all x86_64 targets.
        unsafe {
            let va = _mm_loadu_si128(a.as_ptr() as *const __m128i);
            let vb = _mm_loadu_si128(b.as_ptr() as *const __m128i);
            let cmp = _mm_cmpeq_epi8(va, vb);
            let mask = _mm_movemask_epi8(cmp) as u32;
            if mask != 0xFFFF {
                return (mask.trailing_ones() as usize).min(max_len);
            }
        }
        // All 16 bytes matched; cap at max_len.
        return max_len.min(16);
    }

    // Scalar fallback for short comparisons.
    let mut matched = 0;
    while matched < max_len && a[matched] == b[matched] {
        matched += 1;
    }
    matched
}

/// Scalar fallback for non-x86_64 targets.
#[cfg(not(target_arch = "x86_64"))]
#[inline]
fn match_length_simd(a: &[u8], b: &[u8], max_len: usize) -> usize {
    let mut matched = 0;
    while matched < max_len && a[matched] == b[matched] {
        matched += 1;
    }
    matched
}

// ---------------------------------------------------------------------------
// Decompression
// ---------------------------------------------------------------------------

/// Decompresses a single PalmDoc-compressed text record.
pub fn decompress(input: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(RECORD_SIZE);
    let mut i = 0;

    while i < input.len() {
        let c = input[i];
        i += 1;

        match c {
            0x00 => {
                // Literal null.
                output.push(0x00);
            },
            0x01..=0x08 => {
                // Copy next `c` bytes literally.
                let count = c as usize;
                if i + count > input.len() {
                    return Err(EruditioError::Format(
                        "PalmDoc: literal copy extends past input".into(),
                    ));
                }
                output.extend_from_slice(&input[i..i + count]);
                i += count;
            },
            0x09..=0x7F => {
                // Self-representing byte.
                output.push(c);
            },
            0x80..=0xBF => {
                // LZ77 back-reference: 2-byte encoding.
                if i >= input.len() {
                    return Err(EruditioError::Format(
                        "PalmDoc: back-reference missing second byte".into(),
                    ));
                }
                let next = input[i];
                i += 1;

                let pair = ((c as u16) << 8) | (next as u16);
                let distance = ((pair & 0x3FFF) >> 3) as usize;
                let length = ((pair & 0x07) + 3) as usize;

                if distance == 0 || distance > output.len() {
                    return Err(EruditioError::Format(format!(
                        "PalmDoc: invalid back-reference distance {} (output len: {})",
                        distance,
                        output.len()
                    )));
                }

                let start = output.len() - distance;
                if output.len().saturating_add(length) > MAX_DECOMPRESS_OUTPUT {
                    return Err(EruditioError::Compression(
                        "PalmDoc: decompressed output exceeds size limit".into(),
                    ));
                }

                if distance >= length {
                    // Non-overlapping: bulk copy is safe.
                    // Derive both pointers from as_mut_ptr() to avoid creating
                    // aliasing &/*mut pairs that could violate Stacked Borrows.
                    output.reserve(length);
                    let len = output.len();
                    unsafe {
                        let base = output.as_mut_ptr();
                        let src = base.add(start) as *const u8;
                        let dst = base.add(len);
                        // SAFETY: `start + length <= len` because `distance >= length`
                        // and `start = len - distance`. We reserved `length` bytes so
                        // `dst` is valid for writes. Regions do not overlap.
                        std::ptr::copy_nonoverlapping(src, dst, length);
                        output.set_len(len + length);
                    }
                } else {
                    // Overlapping: byte-by-byte copy (needed for run-length
                    // style references where distance < length).
                    for j in 0..length {
                        let byte = output[start + j];
                        output.push(byte);
                    }
                }
            },
            0xC0..=0xFF => {
                // Space + character.
                output.push(b' ');
                output.push(c ^ 0x80);
            },
        }
    }

    Ok(output)
}

// ---------------------------------------------------------------------------
// Compression
// ---------------------------------------------------------------------------

/// Compresses a single text record using PalmDoc LZ77.
///
/// Input should be at most `RECORD_SIZE` (4096) bytes.
pub fn compress(input: &[u8]) -> Vec<u8> {
    if input.is_empty() {
        return Vec::new();
    }

    let mut output = Vec::with_capacity(input.len());
    let mut chain = HashChain::new();
    let mut i = 0;

    while i < input.len() {
        // Try LZ77 back-reference.
        if i >= MIN_MATCH
            && input.len() - i >= MIN_MATCH
            && let Some((distance, length)) = chain.find_best_match(input, i)
        {
            // Update the hash chain for every position consumed by this match.
            for p in i..i + length {
                chain.insert(input, p);
            }

            let compound = ((distance << 3) | (length - 3)) as u16;
            output.push(0x80 | ((compound >> 8) as u8));
            output.push((compound & 0xFF) as u8);
            i += length;
            continue;
        }

        // Update the hash chain for the current position (even if we don't
        // emit a back-reference).
        chain.insert(input, i);

        // Try space + character optimization.
        if input[i] == b' ' && i + 1 < input.len() {
            let next = input[i + 1];
            if (0x40..=0x7F).contains(&next) {
                // Also insert the position we're about to skip.
                chain.insert(input, i + 1);
                output.push(next ^ 0x80);
                i += 2;
                continue;
            }
        }

        // Self-representing byte.
        if input[i] == 0x00 || (0x09..=0x7F).contains(&input[i]) {
            output.push(input[i]);
            i += 1;
            continue;
        }

        // Binary literal: collect up to 8 bytes that aren't self-representing.
        let start = i;
        let mut count = 0;
        while i < input.len()
            && count < 8
            && !(input[i] == 0x00 || (0x09..=0x7F).contains(&input[i]))
        {
            // Also stop if the next sequence could be a space optimization.
            if input[i] == b' ' && i + 1 < input.len() && (0x40..=0x7F).contains(&input[i + 1]) {
                break;
            }
            // Insert every position we skip past into the hash chain.
            chain.insert(input, i);
            count += 1;
            i += 1;
        }

        if count > 0 {
            output.push(count as u8);
            output.extend_from_slice(&input[start..start + count]);
        } else {
            // Single byte that doesn't fit other categories -- emit as 1-byte literal.
            output.push(1);
            output.push(input[i]);
            i += 1;
        }
    }

    output
}

// ---------------------------------------------------------------------------
// Naive (brute-force) match finder -- kept for test validation.
// ---------------------------------------------------------------------------

/// Brute-force match finder: linear scan of the sliding window. Retained under
/// `#[cfg(test)]` so we can cross-validate the hash-chain implementation.
#[cfg(test)]
fn find_best_match_naive(data: &[u8], pos: usize) -> Option<(usize, usize)> {
    let remaining = data.len() - pos;
    let max_len = remaining.min(MAX_MATCH);
    if max_len < MIN_MATCH {
        return None;
    }

    let window_start = pos.saturating_sub(MAX_DISTANCE);
    let mut best_distance = 0;
    let mut best_length = 0;

    let mut search_pos = window_start;
    while search_pos < pos {
        let mut length = 0;
        while length < max_len && data[search_pos + length] == data[pos + length] {
            length += 1;
        }

        if length >= MIN_MATCH && length > best_length {
            best_distance = pos - search_pos;
            best_length = length;
            if best_length == MAX_MATCH {
                break; // Can't do better.
            }
        }

        search_pos += 1;
    }

    if best_length >= MIN_MATCH {
        Some((best_distance, best_length))
    } else {
        None
    }
}

/// Compresses using the naive brute-force approach (for test comparison).
#[cfg(test)]
fn compress_naive(input: &[u8]) -> Vec<u8> {
    if input.is_empty() {
        return Vec::new();
    }

    let mut output = Vec::with_capacity(input.len());
    let mut i = 0;

    while i < input.len() {
        // Try LZ77 back-reference.
        if i >= MIN_MATCH
            && input.len() - i >= MIN_MATCH
            && let Some((distance, length)) = find_best_match_naive(input, i)
        {
            let compound = ((distance << 3) | (length - 3)) as u16;
            output.push(0x80 | ((compound >> 8) as u8));
            output.push((compound & 0xFF) as u8);
            i += length;
            continue;
        }

        // Try space + character optimization.
        if input[i] == b' ' && i + 1 < input.len() {
            let next = input[i + 1];
            if (0x40..=0x7F).contains(&next) {
                output.push(next ^ 0x80);
                i += 2;
                continue;
            }
        }

        // Self-representing byte.
        if input[i] == 0x00 || (0x09..=0x7F).contains(&input[i]) {
            output.push(input[i]);
            i += 1;
            continue;
        }

        // Binary literal: collect up to 8 bytes that aren't self-representing.
        let start = i;
        let mut count = 0;
        while i < input.len()
            && count < 8
            && !(input[i] == 0x00 || (0x09..=0x7F).contains(&input[i]))
        {
            if input[i] == b' ' && i + 1 < input.len() && (0x40..=0x7F).contains(&input[i + 1]) {
                break;
            }
            count += 1;
            i += 1;
        }

        if count > 0 {
            output.push(count as u8);
            output.extend_from_slice(&input[start..start + count]);
        } else {
            output.push(1);
            output.push(input[i]);
            i += 1;
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompress_empty() {
        let result = decompress(&[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn decompress_literal_bytes() {
        // Self-representing bytes: 'H', 'i'
        let result = decompress(b"Hi").unwrap();
        assert_eq!(result, b"Hi");
    }

    #[test]
    fn decompress_space_char() {
        // 0xC0 | ('t' ^ 0x80) won't work since 't' = 0x74, 0x74 ^ 0x80 = 0xF4
        // Space + 't': output 0x20, 't'
        // Encoding: 0xC0 | 0x74 = ... no, it's: byte = char ^ 0x80 | 0xC0
        // Actually the encoding is: single byte C where C = next_char XOR 0x80
        // and C must be in 0xC0..=0xFF, meaning next_char is in 0x40..=0x7F.
        // 't' = 0x74, so encoded byte = 0x74 ^ 0x80 = 0xF4.
        let input = &[0xF4]; // space + 't'
        let result = decompress(input).unwrap();
        assert_eq!(result, b" t");
    }

    #[test]
    fn decompress_literal_copy() {
        // 0x03 means copy next 3 bytes literally.
        let input = &[0x03, 0x01, 0x02, 0x03, b'X'];
        let result = decompress(input).unwrap();
        assert_eq!(result, &[0x01, 0x02, 0x03, b'X']);
    }

    #[test]
    fn compress_decompress_round_trip_ascii() {
        let original = b"Hello, World! This is a test of the PalmDoc compression system.";
        let compressed = compress(original);
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn compress_decompress_round_trip_repeated() {
        let original = b"abcabc abcabc abcabc abcabc abcabc";
        let compressed = compress(original);
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(decompressed, original);
        // Repeated data should compress smaller.
        assert!(compressed.len() < original.len());
    }

    #[test]
    fn compress_decompress_round_trip_binary() {
        let mut original = Vec::with_capacity(256);
        for i in 0..=255u8 {
            original.push(i);
        }
        let compressed = compress(&original);
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn compress_empty() {
        let compressed = compress(b"");
        assert!(compressed.is_empty());
    }

    #[test]
    fn compress_space_optimization() {
        // "Hello World" -- the space before 'W' (0x57, in 0x40..0x7F) should compress.
        let original = b"Hello World";
        let compressed = compress(original);
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(decompressed, original);
        // " W" compresses to 1 byte (0x57 ^ 0x80 = 0xD7), so output should be shorter.
        assert!(compressed.len() < original.len());
    }

    #[test]
    fn round_trip_full_record() {
        // Simulate a full 4096-byte record.
        let mut original = Vec::with_capacity(RECORD_SIZE);
        let phrase = b"The quick brown fox jumps over the lazy dog. ";
        while original.len() + phrase.len() <= RECORD_SIZE {
            original.extend_from_slice(phrase);
        }
        while original.len() < RECORD_SIZE {
            original.push(b'X');
        }

        let compressed = compress(&original);
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(decompressed, original);
        // Highly repetitive text should compress well.
        assert!(compressed.len() < original.len() / 2);
    }

    /// Validates that the hash-chain compressor produces valid output that
    /// round-trips correctly, and achieves comparable (or better) compression
    /// to the naive brute-force approach.
    #[test]
    fn hash_chain_vs_naive_compression() {
        // Build a realistic 4096-byte record with repetitive content.
        let mut original = Vec::with_capacity(RECORD_SIZE);
        let phrases: &[&[u8]] = &[
            b"The quick brown fox jumps over the lazy dog. ",
            b"Lorem ipsum dolor sit amet, consectetur adipiscing elit. ",
            b"abcdefghij abcdefghij abcdefghij ",
        ];
        let mut idx = 0;
        while original.len() + phrases[idx % phrases.len()].len() <= RECORD_SIZE {
            original.extend_from_slice(phrases[idx % phrases.len()]);
            idx += 1;
        }
        while original.len() < RECORD_SIZE {
            original.push(b'.');
        }

        let compressed_hash = compress(&original);
        let compressed_naive = compress_naive(&original);

        // Both must round-trip correctly.
        let decompressed_hash = decompress(&compressed_hash).unwrap();
        let decompressed_naive = decompress(&compressed_naive).unwrap();
        assert_eq!(decompressed_hash, original, "hash-chain round-trip failed");
        assert_eq!(decompressed_naive, original, "naive round-trip failed");

        // Hash-chain should produce output of similar or better size.
        // Allow up to 10% worse in case the hash chain misses a few matches
        // that the exhaustive scan found.
        let tolerance = (compressed_naive.len() as f64 * 1.10) as usize;
        assert!(
            compressed_hash.len() <= tolerance,
            "hash-chain compressed size ({}) is more than 10% worse than naive ({})",
            compressed_hash.len(),
            compressed_naive.len(),
        );
    }
}
