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

/// Maximum LZ77 back-reference distance.
const MAX_DISTANCE: usize = 2047;

/// Minimum LZ77 match length.
const MIN_MATCH: usize = 3;

/// Maximum LZ77 match length.
const MAX_MATCH: usize = 10;

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
            }
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
            }
            0x09..=0x7F => {
                // Self-representing byte.
                output.push(c);
            }
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
                // Copy byte-by-byte to handle overlapping references.
                for j in 0..length {
                    let byte = output[start + j];
                    output.push(byte);
                }
            }
            0xC0..=0xFF => {
                // Space + character.
                output.push(b' ');
                output.push(c ^ 0x80);
            }
        }
    }

    Ok(output)
}

/// Compresses a single text record using PalmDoc LZ77.
///
/// Input should be at most `RECORD_SIZE` (4096) bytes.
pub fn compress(input: &[u8]) -> Vec<u8> {
    if input.is_empty() {
        return Vec::new();
    }

    let mut output = Vec::with_capacity(input.len());
    let mut i = 0;

    while i < input.len() {
        // Try LZ77 back-reference.
        if i >= MIN_MATCH
            && input.len() - i >= MIN_MATCH
            && let Some((distance, length)) = find_best_match(input, i)
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
            // Also stop if the next sequence could be a space optimization.
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
            // Single byte that doesn't fit other categories — emit as 1-byte literal.
            output.push(1);
            output.push(input[i]);
            i += 1;
        }
    }

    output
}

/// Finds the longest match in the sliding window for LZ77 compression.
///
/// Returns `(distance, length)` if a match of at least `MIN_MATCH` is found.
fn find_best_match(data: &[u8], pos: usize) -> Option<(usize, usize)> {
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
        // "Hello World" — the space before 'W' (0x57, in 0x40..0x7F) should compress.
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
}
