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
///
/// This struct is 16 KB. To avoid placing it on the stack, the
/// [`PalmDocCompressor`] wraps it in a `Box` for heap allocation and reuse.
struct HashChain {
    head: [u16; HASH_SIZE],
    prev: [u16; RECORD_SIZE],
}

impl HashChain {
    /// Resets the hash chain for reuse without reallocating.
    fn reset(&mut self) {
        self.head.fill(NO_ENTRY);
        self.prev.fill(NO_ENTRY);
    }

    /// Creates a new, heap-allocated hash chain.
    fn new_boxed() -> Box<Self> {
        let mut chain = Box::new(Self {
            head: [0u16; HASH_SIZE],
            prev: [0u16; RECORD_SIZE],
        });
        chain.reset();
        chain
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

            let length = crate::formats::common::intrinsics::match_length::common_prefix_length(
                &data[cand..],
                &data[pos..],
                max_len,
            );

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
// Decompression
// ---------------------------------------------------------------------------

/// Decompresses a single PalmDoc-compressed text record.
pub fn decompress(input: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(RECORD_SIZE);
    decompress_into(input, &mut output)?;
    Ok(output)
}

/// Decompresses a single PalmDoc-compressed text record, appending directly
/// to the provided buffer. Avoids allocating a temporary `Vec` per record.
pub fn decompress_into(input: &[u8], output: &mut Vec<u8>) -> Result<()> {
    output.reserve(RECORD_SIZE);
    let mut i = 0;
    let len = input.len();

    while i < len {
        let c = input[i];

        // Fast path: scan for a run of self-representing bytes (0x09..=0x7F).
        // This is the overwhelmingly common case for text-heavy ebook content
        // (plain ASCII letters, digits, punctuation). We bulk-copy entire runs
        // instead of pushing one byte at a time.
        if c >= 0x09 && c <= 0x7F {
            let run_start = i;
            i += 1;
            while i < len {
                let b = input[i];
                if b >= 0x09 && b <= 0x7F {
                    i += 1;
                } else {
                    break;
                }
            }
            output.extend_from_slice(&input[run_start..i]);
            continue;
        }

        i += 1;

        match c {
            0x00 => {
                // Literal null.
                output.push(0x00);
            },
            0x01..=0x08 => {
                // Copy next `c` bytes literally.
                let count = c as usize;
                if i + count > len {
                    return Err(EruditioError::Format(
                        "PalmDoc: literal copy extends past input".into(),
                    ));
                }
                output.extend_from_slice(&input[i..i + count]);
                i += count;
            },
            0x09..=0x7F => {
                // Self-representing bytes are handled above in the fast path.
                // This arm is unreachable but required for exhaustiveness.
                unreachable!()
            },
            0x80..=0xBF => {
                // LZ77 back-reference: 2-byte encoding.
                if i >= len {
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
                    output.reserve(length);
                    let out_len = output.len();
                    unsafe {
                        let base = output.as_mut_ptr();
                        let src = base.add(start) as *const u8;
                        let dst = base.add(out_len);
                        // SAFETY: `start + length <= out_len` because `distance >= length`
                        // and `start = out_len - distance`. We reserved `length` bytes so
                        // `dst` is valid for writes. Regions do not overlap.
                        std::ptr::copy_nonoverlapping(src, dst, length);
                        output.set_len(out_len + length);
                    }
                } else if distance == 1 {
                    // RLE: single-byte repeat -- the most common overlapping case.
                    let byte = output[output.len() - 1];
                    output.extend(std::iter::repeat_n(byte, length));
                } else {
                    // Overlapping: the source pattern of `distance` bytes
                    // repeats to fill `length` bytes. Copy in doubling chunks
                    // to amortise the per-byte overhead.
                    output.reserve(length);
                    let base_len = output.len();
                    // Seed: copy the initial `distance` bytes one by one
                    // (they overlap with the source being built).
                    let seed = distance.min(length);
                    for j in 0..seed {
                        let byte = output[start + j];
                        output.push(byte);
                    }
                    // Double up: memcpy from already-written output in
                    // power-of-two chunks until we reach `length`.
                    let mut written = seed;
                    while written < length {
                        let chunk = (length - written).min(written);
                        let src_start = base_len;
                        // SAFETY: `src_start + chunk <= output.len()` because
                        // we have already pushed `written >= chunk` bytes
                        // starting at `base_len`. `output.len() + chunk` is
                        // within the reserved capacity.
                        unsafe {
                            let ptr = output.as_mut_ptr();
                            std::ptr::copy_nonoverlapping(
                                ptr.add(src_start),
                                ptr.add(base_len + written),
                                chunk,
                            );
                            output.set_len(base_len + written + chunk);
                        }
                        written += chunk;
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

    Ok(())
}

// ---------------------------------------------------------------------------
// Compression
// ---------------------------------------------------------------------------

/// Compresses a single text record using PalmDoc LZ77.
///
/// Input should be at most `RECORD_SIZE` (4096) bytes.
///
/// For compressing multiple records, prefer [`PalmDocCompressor`] which
/// amortises the 16 KB `HashChain` allocation and initialisation cost.
pub fn compress(input: &[u8]) -> Vec<u8> {
    PalmDocCompressor::new().compress_record(input)
}

// ---------------------------------------------------------------------------
// Reusable compressor (avoids 16 KB HashChain re-init per record)
// ---------------------------------------------------------------------------

/// A reusable PalmDoc compressor that amortises the 16 KB `HashChain`
/// initialisation cost across multiple records.  For a typical MOBI book
/// with 50 text records, this eliminates 50 x 16 KB = 800 KB of memset.
///
/// The `HashChain` is heap-allocated via `Box` to avoid placing 16 KB on
/// the stack.
pub struct PalmDocCompressor {
    chain: Box<HashChain>,
}

impl Default for PalmDocCompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl PalmDocCompressor {
    /// Creates a new compressor with an initialized hash chain.
    pub fn new() -> Self {
        Self {
            chain: HashChain::new_boxed(),
        }
    }

    /// Compresses a single text record, reusing the internal hash chain.
    pub fn compress_record(&mut self, input: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(input.len());
        self.compress_record_into(input, &mut output);
        output
    }

    /// Compresses a single text record into the given output buffer, which is
    /// cleared first. Reuses both the internal hash chain and the caller's
    /// output buffer to eliminate per-record allocation.
    pub fn compress_record_into(&mut self, input: &[u8], output: &mut Vec<u8>) {
        output.clear();
        if input.is_empty() {
            return;
        }

        self.chain.reset();
        output.reserve(input.len());
        let input_len = input.len();
        let mut i = 0;

        while i < input_len {
            // Try LZ77 back-reference.
            if i >= MIN_MATCH
                && input_len - i >= MIN_MATCH
                && let Some((distance, length)) = self.chain.find_best_match(input, i)
            {
                // Update the hash chain for every position consumed by this match.
                for p in i..i + length {
                    self.chain.insert(input, p);
                }

                let compound = ((distance << 3) | (length - 3)) as u16;
                output.push(0x80 | ((compound >> 8) as u8));
                output.push((compound & 0xFF) as u8);
                i += length;
                continue;
            }

            // Update the hash chain for the current position.
            self.chain.insert(input, i);

            // Try space + character optimization.
            if input[i] == b' ' && i + 1 < input_len {
                let next = input[i + 1];
                if (0x40..=0x7F).contains(&next) {
                    self.chain.insert(input, i + 1);
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
            while i < input_len
                && count < 8
                && !(input[i] == 0x00 || (0x09..=0x7F).contains(&input[i]))
            {
                if input[i] == b' '
                    && i + 1 < input_len
                    && (0x40..=0x7F).contains(&input[i + 1])
                {
                    break;
                }
                self.chain.insert(input, i);
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
    }
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
        let original = b"Hello World";
        let compressed = compress(original);
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(decompressed, original);
        assert!(compressed.len() < original.len());
    }

    #[test]
    fn decompress_overlapping_backref() {
        let input = &[b'a', b'b', b'c', 0x80, 0x1E];
        let result = decompress(input).unwrap();
        assert_eq!(result, b"abcabcabcabc"); // 3 original + 9 from backref = 12
    }

    #[test]
    fn palmdoc_compressor_reuse() {
        let mut compressor = PalmDocCompressor::new();

        let phrases: &[&[u8]] = &[
            b"The quick brown fox jumps over the lazy dog. ",
            b"Lorem ipsum dolor sit amet, consectetur adipiscing elit. ",
            b"abcabc abcabc abcabc abcabc abcabc",
        ];

        for phrase in phrases {
            let compressed = compressor.compress_record(phrase);
            let decompressed = decompress(&compressed).unwrap();
            assert_eq!(&decompressed, phrase);
        }

        // Also test a full-size record.
        let mut record = Vec::with_capacity(RECORD_SIZE);
        while record.len() + phrases[0].len() <= RECORD_SIZE {
            record.extend_from_slice(phrases[0]);
        }
        let compressed = compressor.compress_record(&record);
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(decompressed, record);
    }

    #[test]
    fn compress_record_into_round_trip() {
        let mut compressor = PalmDocCompressor::new();
        let mut buf = Vec::new();

        let phrases: &[&[u8]] = &[
            b"The quick brown fox jumps over the lazy dog. ",
            b"Lorem ipsum dolor sit amet, consectetur adipiscing elit. ",
            b"abcabc abcabc abcabc abcabc abcabc",
        ];

        for phrase in phrases {
            compressor.compress_record_into(phrase, &mut buf);
            let decompressed = decompress(&buf).unwrap();
            assert_eq!(&decompressed, phrase);
        }
    }

    #[test]
    fn round_trip_full_record() {
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
        assert!(compressed.len() < original.len() / 2);
    }

    #[test]
    fn hash_chain_vs_naive_compression() {
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

        let decompressed_hash = decompress(&compressed_hash).unwrap();
        let decompressed_naive = decompress(&compressed_naive).unwrap();
        assert_eq!(decompressed_hash, original, "hash-chain round-trip failed");
        assert_eq!(decompressed_naive, original, "naive round-trip failed");

        let tolerance = (compressed_naive.len() as f64 * 1.10) as usize;
        assert!(
            compressed_hash.len() <= tolerance,
            "hash-chain compressed size ({}) is more than 10% worse than naive ({})",
            compressed_hash.len(),
            compressed_naive.len(),
        );
    }
}
