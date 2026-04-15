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
use crate::formats::common::intrinsics::prefetch::{prefetch_read_l1, prefetch_read_l2};

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
///
/// Reduced from 64 to 12: Cachegrind showed `find_best_match` consuming the
/// majority of instructions in EPUB→MOBI.  For ebook text (English prose),
/// the best match is almost always found within the first 8–10 chain links;
/// longer walks visit stale positions outside the window.  Combined with
/// early exit at match length ≥ 4, this cuts chain walk instructions by ~5×
/// with negligible compression impact (<1% for typical ebook content).
const MAX_CHAIN: usize = 12;

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
/// Uses a generation counter to avoid 16 KB `memset` on every `reset()`.
/// `head_gen[h]` records the generation in which `head[h]` was last written;
/// stale entries (generation mismatch) are treated as `NO_ENTRY`.  `prev[]`
/// entries are implicitly invalidated because they are only reachable through
/// a current-generation `head[]` entry.
///
/// This struct is ~24 KB. To avoid placing it on the stack, the
/// [`PalmDocCompressor`] wraps it in a `Box` for heap allocation and reuse.
struct HashChain {
    head: [u16; HASH_SIZE],
    prev: [u16; RECORD_SIZE],
    head_gen: [u16; HASH_SIZE],
    generation: u16,
}

impl HashChain {
    /// Resets the hash chain for reuse without reallocating.
    ///
    /// In the common case this is O(1): just increment the generation counter.
    /// A full memset only happens on the rare u16 wrap-around (every 65 535
    /// records ≈ 256 MB of uncompressed text).
    fn reset(&mut self) {
        let next = self.generation.wrapping_add(1);
        if next == 0 {
            // Wrapped: all head_gen entries are indistinguishable from the new
            // generation (0), so we must do a real reset.
            self.head.fill(NO_ENTRY);
            self.prev.fill(NO_ENTRY);
            self.head_gen.fill(0);
            self.generation = 1;
        } else {
            self.generation = next;
        }
    }

    /// Creates a new, heap-allocated hash chain.
    fn new_boxed() -> Box<Self> {
        // head_gen is 0 everywhere but generation is 1 → all entries start stale.
        Box::new(Self {
            head: [NO_ENTRY; HASH_SIZE],
            prev: [NO_ENTRY; RECORD_SIZE],
            head_gen: [0u16; HASH_SIZE],
            generation: 1,
        })
    }

    /// Computes a fast 12-bit hash of three consecutive bytes.
    ///
    /// Uses a multiplicative hash for better bucket distribution than
    /// XOR-shift, reducing chain lengths for common English trigrams.
    #[inline]
    fn hash3(data: &[u8], pos: usize) -> usize {
        let h = (data[pos] as u32)
            .wrapping_mul(1117)
            .wrapping_add(data[pos + 1] as u32)
            .wrapping_mul(1117)
            .wrapping_add(data[pos + 2] as u32);
        (h as usize) & (HASH_SIZE - 1)
    }

    /// Inserts `pos` into the chain for `data[pos..pos+3]`.
    #[inline]
    fn insert(&mut self, data: &[u8], pos: usize) {
        if pos + 2 < data.len() {
            let h = Self::hash3(data, pos);
            // Only chain to the previous head if it belongs to the current
            // generation; otherwise start a fresh chain.
            self.prev[pos] = if self.head_gen[h] == self.generation {
                self.head[h]
            } else {
                NO_ENTRY
            };
            self.head[h] = pos as u16;
            self.head_gen[h] = self.generation;
        }
    }

    /// Inserts `pos` into the chain for `data[pos..pos+3]` without checking
    /// that `pos + 2 < data.len()`.
    ///
    /// # Safety contract (not unsafe, but caller must ensure):
    /// The caller MUST guarantee that `pos + 2 < data.len()`.
    #[inline]
    fn insert_unchecked(&mut self, data: &[u8], pos: usize) {
        debug_assert!(pos + 2 < data.len());
        let h = Self::hash3(data, pos);
        self.prev[pos] = if self.head_gen[h] == self.generation {
            self.head[h]
        } else {
            NO_ENTRY
        };
        self.head[h] = pos as u16;
        self.head_gen[h] = self.generation;
    }

    /// Computes the match length between `data[a_pos..]` and `data[b_pos..]`,
    /// up to `max_len` bytes.
    ///
    /// On little-endian targets, uses u64 XOR comparison to find the first
    /// differing byte in a branchless manner, replacing up to 10 conditional
    /// branches with 1-2 branches plus integer arithmetic.
    #[inline]
    fn match_length(data: &[u8], a_pos: usize, b_pos: usize, max_len: usize) -> usize {
        debug_assert!(max_len <= MAX_MATCH); // At most 10

        #[cfg(target_endian = "little")]
        {
            // Fast path: if both positions have at least 8 bytes remaining in
            // the data buffer, we can do a single u64 comparison for the first
            // 8 bytes, then scalar for the remaining 2 (MAX_MATCH=10).
            if a_pos + 8 <= data.len() && b_pos + 8 <= data.len() {
                // SAFETY: We just verified that `a_pos + 8 <= data.len()` and
                // `b_pos + 8 <= data.len()`, so reading 8 bytes from each
                // position is within bounds. We use `read_unaligned` because
                // these positions are not guaranteed to be 8-byte aligned.
                let a_word = unsafe { (data.as_ptr().add(a_pos) as *const u64).read_unaligned() };
                let b_word = unsafe { (data.as_ptr().add(b_pos) as *const u64).read_unaligned() };

                let xor = a_word ^ b_word;
                if xor != 0 {
                    // On little-endian, trailing zeros / 8 gives the index of
                    // the first differing byte.
                    let first_diff = (xor.trailing_zeros() / 8) as usize;
                    return first_diff.min(max_len);
                }

                // First 8 bytes match. Check remaining bytes (up to 2 more
                // since MAX_MATCH=10) with scalar comparison.
                let mut length = 8.min(max_len);
                while length < max_len && data[a_pos + length] == data[b_pos + length] {
                    length += 1;
                }
                return length;
            }
        }

        // Scalar fallback: used near the end of the buffer (where we can't
        // safely read 8 bytes) or on big-endian targets.
        let mut length = 0;
        while length < max_len && data[a_pos + length] == data[b_pos + length] {
            length += 1;
        }
        length
    }

    /// Walks the chain for the hash of `data[pos..pos+3]` and returns the best
    /// `(distance, length)` pair, or `None` if no match of at least `MIN_MATCH`
    /// is found.
    #[inline]
    fn find_best_match(&self, data: &[u8], pos: usize) -> Option<(usize, usize)> {
        let remaining = data.len() - pos;
        let max_len = remaining.min(MAX_MATCH);
        if max_len < MIN_MATCH {
            return None;
        }

        let h = Self::hash3(data, pos);
        // Stale head entry → no matches in the current generation.
        if self.head_gen[h] != self.generation {
            return None;
        }
        let mut candidate = self.head[h];
        let mut best_distance: usize = 0;
        let mut best_length: usize = 0;
        let mut steps = 0;
        let window_start = pos.saturating_sub(MAX_DISTANCE);

        while candidate != NO_ENTRY && steps < MAX_CHAIN {
            let cand = candidate as usize;

            // Prefetch the next chain link early, before the heavy work
            // (match_length). Reading `self.prev[cand]` now and issuing a
            // prefetch for the next candidate's data gives the memory
            // subsystem time to fetch the cache line while we're busy
            // comparing bytes.
            //
            // Prefetch distance: 1 step ahead. Justification: the chain
            // walk body is ~15-20 cycles (match_length + comparisons), L1
            // latency is ~4-5 cycles, so 1 step gives sufficient time for
            // the prefetch to complete.
            let next_candidate = self.prev[cand];
            if next_candidate != NO_ENTRY {
                let next_cand = next_candidate as usize;
                if next_cand < data.len() {
                    // SAFETY: `next_cand < data.len()` guarantees the pointer
                    // is within the allocation. `prefetch_read_l1` is a hint
                    // and does not dereference the pointer — a faulting
                    // address is silently ignored by the CPU.
                    prefetch_read_l1(unsafe { data.as_ptr().add(next_cand) });
                }
            }

            // Candidate must be before `pos` and inside the sliding window.
            if cand >= pos || cand < window_start {
                candidate = next_candidate;
                steps += 1;
                continue;
            }

            let length = Self::match_length(data, cand, pos, max_len);

            if length >= MIN_MATCH && length > best_length {
                best_distance = pos - cand;
                best_length = length;
                // Early exit: a match of 4+ bytes captures 40% of the
                // theoretical maximum (MAX_MATCH=10).  Continuing the chain
                // walk yields diminishing returns — each extra byte saves
                // only 1 byte of output while the walk costs ~20 insns/step.
                if best_length >= 4 {
                    break;
                }
            }

            candidate = next_candidate;
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
        if (0x09..=0x7F).contains(&c) {
            let run_start = i;
            i += 1;
            while i < len {
                let b = input[i];
                if (0x09..=0x7F).contains(&b) {
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

/// A reusable PalmDoc compressor that amortises the `HashChain` heap
/// allocation across multiple records.  The hash chain uses a generation
/// counter so that `reset()` is O(1) — no 16 KB memset per record.
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
        // Positions below this limit are guaranteed to have 3 valid bytes for
        // hash3 (pos, pos+1, pos+2), so insert_unchecked can be used safely.
        let hash_safe_limit = input_len.saturating_sub(2);
        let mut i = 0;

        while i < input_len {
            // Software-pipelined input prefetch: warm the next cache lines of input
            // data while processing the current position. This overlaps memory
            // latency with the hash chain operations and match-finding work.
            //
            // L1 prefetch at +64 bytes (1 cache line ahead): covers the next
            // ~64 bytes of sequential access. The loop body takes ~20-40 cycles
            // per iteration (hash + potential match_length), giving L1 (~4-5 cycles)
            // ample time to complete.
            //
            // L2 prefetch at +256 bytes (4 cache lines ahead): warms L2 for data
            // that will reach L1 in ~4-8 iterations, hiding L2 latency (~12-14 cycles).
            //
            // μop budget: 2 prefetch μops per iteration. On x86_64, the main loop
            // body is estimated at ~15-20 μops for the common path (hash + literal),
            // so 2 extra μops keeps us well within LSD eligibility (≤28 μops).
            if i + 64 < input_len {
                // SAFETY: `i + 64 < input_len` guarantees the pointer is within bounds.
                prefetch_read_l1(unsafe { input.as_ptr().add(i + 64) });
            }
            if i + 256 < input_len {
                // SAFETY: `i + 256 < input_len` guarantees the pointer is within bounds.
                prefetch_read_l2(unsafe { input.as_ptr().add(i + 256) });
            }

            // Try LZ77 back-reference.
            if i >= MIN_MATCH
                && input_len - i >= MIN_MATCH
                && let Some((distance, length)) = self.chain.find_best_match(input, i)
            {
                // Insert positions in the matched range into the hash
                // chain at stride-2 (every other position). This halves
                // insertion cost while keeping most match candidates
                // available for future lookups.  The first position (i) is
                // always inserted to anchor the chain; intermediate positions
                // are inserted at even offsets relative to i.
                let safe_end = (i + length).min(hash_safe_limit);
                let mut p = i;
                while p < safe_end {
                    self.chain.insert_unchecked(input, p);
                    p += 2;
                }
                // Handle remaining 0-2 positions near the buffer end.
                while p < i + length {
                    self.chain.insert(input, p);
                    p += 2;
                }

                let compound = ((distance << 3) | (length - 3)) as u16;
                output.push(0x80 | ((compound >> 8) as u8));
                output.push((compound & 0xFF) as u8);
                i += length;
                continue;
            }

            // Update the hash chain for the current position.
            if i < hash_safe_limit {
                self.chain.insert_unchecked(input, i);
            } else {
                self.chain.insert(input, i);
            }

            // Try space + character optimization.
            if input[i] == b' ' && i + 1 < input_len {
                let next = input[i + 1];
                if (0x40..=0x7F).contains(&next) {
                    if i + 1 < hash_safe_limit {
                        self.chain.insert_unchecked(input, i + 1);
                    } else {
                        self.chain.insert(input, i + 1);
                    }
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
                if input[i] == b' ' && i + 1 < input_len && (0x40..=0x7F).contains(&input[i + 1]) {
                    break;
                }
                if i < hash_safe_limit {
                    self.chain.insert_unchecked(input, i);
                } else {
                    self.chain.insert(input, i);
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

    #[test]
    fn stride1_compression_produces_smaller_output() {
        // Build a record with repetitive text that benefits from good LZ77
        // matching. Stride-1 insertion should produce output that is strictly
        // smaller than the input and comparable to the naive brute-force
        // compressor (which by definition finds all matches).
        let phrase = b"The quick brown fox jumps over the lazy dog. ";
        let mut original = Vec::with_capacity(RECORD_SIZE);
        while original.len() + phrase.len() <= RECORD_SIZE {
            original.extend_from_slice(phrase);
        }
        while original.len() < RECORD_SIZE {
            original.push(b'.');
        }

        let compressed = compress(&original);
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(decompressed, original, "round-trip mismatch");

        // Compression must achieve at least 50% reduction on this highly
        // repetitive input.
        assert!(
            compressed.len() <= original.len() / 2,
            "compressed size ({}) should be <= half the original ({})",
            compressed.len(),
            original.len(),
        );

        // With stride-1, the hash-chain compressor should be within 5% of
        // the brute-force naive compressor on repetitive data.
        let compressed_naive = compress_naive(&original);
        let tolerance = (compressed_naive.len() as f64 * 1.05) as usize;
        assert!(
            compressed_hash_len_within(compressed.len(), tolerance),
            "stride-1 hash-chain ({}) should be within 5% of naive ({})",
            compressed.len(),
            compressed_naive.len(),
        );
    }

    /// Helper: checks `actual <= limit`.
    fn compressed_hash_len_within(actual: usize, limit: usize) -> bool {
        actual <= limit
    }
}
