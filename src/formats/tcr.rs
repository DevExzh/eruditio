use crate::domain::{Book, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use crate::formats::common::MAX_INPUT_SIZE;
use crate::formats::common::intrinsics;
use crate::formats::txt::{TxtReader, book_to_plain_text};
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

/// TCR format reader.
#[derive(Default)]
pub struct TcrReader;

impl TcrReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for TcrReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut buffer = Vec::new();
        (&mut *reader)
            .take(MAX_INPUT_SIZE)
            .read_to_end(&mut buffer)?;

        if buffer.len() < 9 || &buffer[0..9] != b"!!8-Bit!!" {
            return Err(EruditioError::Format("Invalid TCR header".into()));
        }

        let mut pos = 9;
        let mut entries = Vec::with_capacity(256);

        for _ in 0..256 {
            if pos >= buffer.len() {
                return Err(EruditioError::Format(
                    "Unexpected EOF reading TCR dictionary".into(),
                ));
            }
            let entry_len = buffer[pos] as usize;
            pos += 1;

            if pos + entry_len > buffer.len() {
                return Err(EruditioError::Format(
                    "Unexpected EOF reading TCR dictionary entry".into(),
                ));
            }

            let entry = &buffer[pos..pos + entry_len];
            entries.push(entry.to_vec());
            pos += entry_len;
        }

        let mut decompressed = Vec::new();
        while pos < buffer.len() {
            let index = buffer[pos] as usize;
            if index < entries.len() {
                decompressed.extend_from_slice(&entries[index]);
            }
            pos += 1;
        }

        let mut cursor = Cursor::new(decompressed);
        let mut book = TxtReader::new().read_book(&mut cursor)?;
        book.metadata.title = Some("Unknown TCR Document".into());
        Ok(book)
    }
}

/// TCR format writer.
#[derive(Default)]
pub struct TcrWriter;

impl TcrWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for TcrWriter {
    fn write_book(&self, book: &Book, writer: &mut dyn Write) -> Result<()> {
        let text = book_to_plain_text(book);
        let data = text.as_bytes();

        let (dictionary, encoded) = tcr_compress(data);

        // Write header.
        writer.write_all(b"!!8-Bit!!")?;

        // Write 256 dictionary entries.
        for entry in &dictionary {
            let len = entry.len().min(255) as u8;
            writer.write_all(&[len])?;
            writer.write_all(&entry[..len as usize])?;
        }

        // Write compressed data.
        writer.write_all(&encoded)?;

        Ok(())
    }
}

/// Builds a 256-entry dictionary and encodes `data` using it.
///
/// Strategy: fill the dictionary with the 256 most frequent byte values
/// (as single-byte entries), then greedily encode. If there are fewer than
/// 256 unique bytes, the remaining slots get frequent byte-pairs.
fn tcr_compress(data: &[u8]) -> (Vec<Vec<u8>>, Vec<u8>) {
    if data.is_empty() {
        let dictionary = (0..256).map(|i| vec![i as u8]).collect();
        return (dictionary, Vec::new());
    }

    // Count single-byte frequencies using the multi-array histogram intrinsic.
    let hist = intrinsics::histogram::byte_histogram(data);
    let mut byte_freq = [0u64; 256];
    for (i, &count) in hist.iter().enumerate() {
        byte_freq[i] = count as u64;
    }

    // Collect unique bytes sorted by frequency (most frequent first).
    let mut unique_bytes: Vec<u8> = (0u8..=255).filter(|&b| byte_freq[b as usize] > 0).collect();
    unique_bytes.sort_by(|a, b| byte_freq[*b as usize].cmp(&byte_freq[*a as usize]));

    let mut dictionary: Vec<Vec<u8>> = Vec::with_capacity(256);
    let mut entry_map: HashMap<Vec<u8>, u8> = HashMap::new();

    // Fill with unique single bytes first.
    for &b in &unique_bytes {
        if dictionary.len() >= 256 {
            break;
        }
        let idx = dictionary.len() as u8;
        dictionary.push(vec![b]);
        entry_map.insert(vec![b], idx);
    }

    // If we have remaining slots, add frequent byte-pairs.
    if dictionary.len() < 256 && data.len() >= 2 {
        let mut pair_freq: HashMap<[u8; 2], u64> = HashMap::new();
        for pair in data.windows(2) {
            *pair_freq.entry([pair[0], pair[1]]).or_default() += 1;
        }

        let mut pairs: Vec<([u8; 2], u64)> = pair_freq.into_iter().collect();
        pairs.sort_by(|a, b| b.1.cmp(&a.1));

        for (pair, _freq) in pairs {
            if dictionary.len() >= 256 {
                break;
            }
            let key = pair.to_vec();
            if !entry_map.contains_key(&key) {
                let idx = dictionary.len() as u8;
                entry_map.insert(key.clone(), idx);
                dictionary.push(key);
            }
        }
    }

    // Pad to exactly 256 entries.
    while dictionary.len() < 256 {
        dictionary.push(Vec::new());
    }

    // Greedy encode: try longest match first (2 bytes, then 1 byte).
    let mut encoded = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if i + 1 < data.len() {
            let pair = vec![data[i], data[i + 1]];
            if let Some(&idx) = entry_map.get(&pair) {
                encoded.push(idx);
                i += 2;
                continue;
            }
        }
        if let Some(&idx) = entry_map.get(&data[i..i + 1]) {
            encoded.push(idx);
        }
        i += 1;
    }

    (dictionary, encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;

    #[test]
    fn tcr_round_trip() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: Some("Test".into()),
            content: "<p>Hello World, this is a test of TCR encoding.</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        TcrWriter::new().write_book(&book, &mut output).unwrap();

        // Verify header.
        assert_eq!(&output[..9], b"!!8-Bit!!");

        // Read it back.
        let mut cursor = Cursor::new(output);
        let decoded = TcrReader::new().read_book(&mut cursor).unwrap();
        let chapters = decoded.chapters();
        assert!(!chapters.is_empty());
    }

    #[test]
    fn tcr_compress_empty() {
        let (dict, encoded) = tcr_compress(b"");
        assert_eq!(dict.len(), 256);
        assert!(encoded.is_empty());
    }

    #[test]
    fn tcr_compress_decompresses_correctly() {
        let input = b"Hello Hello Hello World World";
        let (dict, encoded) = tcr_compress(input);

        // Decompress and verify.
        let mut decompressed = Vec::new();
        for &idx in &encoded {
            decompressed.extend_from_slice(&dict[idx as usize]);
        }
        assert_eq!(decompressed, input);
    }
}
