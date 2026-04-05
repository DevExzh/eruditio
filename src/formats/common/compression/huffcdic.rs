//! HUFF/CDIC decompression for MOBI files.
//!
//! Some older Kindle/MOBI books use Huffman-based compression (type 0x4448 = 'DH')
//! instead of the more common PalmDoc LZ77. The compressed data uses a HUFF record
//! containing Huffman tables and one or more CDIC records containing phrase
//! dictionaries.
//!
//! Algorithm ported from calibre's `ebooks/mobi/huffcdic.py` (credit: darkninja, igorsk).

use crate::error::{EruditioError, Result};

/// Maximum recursion depth for dictionary entry decompression.
const MAX_DEPTH: usize = 32;

/// Maximum total decompressed output size to prevent decompression bombs.
const MAX_UNPACK_OUTPUT: usize = 64 * 1024 * 1024; // 64 MB

/// A single entry in dict1 (the fast-lookup table).
#[derive(Clone, Copy)]
struct Dict1Entry {
    code_len: u32,
    terminal: bool,
    max_code: u32,
}

/// HUFF/CDIC decompressor state.
///
/// Constructed from a HUFF record and one or more CDIC records, then
/// used to decompress individual text records.
pub struct HuffCdicReader {
    dict1: [Dict1Entry; 256],
    min_code: Vec<u32>,
    max_code: Vec<u32>,
    dictionary: Vec<DictEntry>,
}

#[derive(Clone)]
enum DictEntry {
    Compressed(Vec<u8>),
    Decompressed(Vec<u8>),
}

impl HuffCdicReader {
    /// Creates a new decompressor from HUFF and CDIC record data.
    ///
    /// `huff` is the raw HUFF record bytes. `cdics` is a slice of raw CDIC records.
    pub fn new(huff: &[u8], cdics: &[&[u8]]) -> Result<Self> {
        let mut reader = Self {
            dict1: [Dict1Entry {
                code_len: 0,
                terminal: false,
                max_code: 0,
            }; 256],
            min_code: Vec::new(),
            max_code: Vec::new(),
            dictionary: Vec::new(),
        };
        reader.load_huff(huff)?;
        for cdic in cdics {
            reader.load_cdic(cdic)?;
        }
        Ok(reader)
    }

    /// Parses the HUFF record to build the dict1 and min_code/max_code tables.
    fn load_huff(&mut self, huff: &[u8]) -> Result<()> {
        if huff.len() < 24 || &huff[0..8] != b"HUFF\x00\x00\x00\x18" {
            return Err(EruditioError::Format("Invalid HUFF header".into()));
        }

        let off1 = read_u32_be(huff, 8) as usize;
        let off2 = read_u32_be(huff, 12) as usize;

        // dict1: 256 packed u32 entries at off1.
        let dict1_end = off1
            .checked_add(256 * 4)
            .ok_or_else(|| EruditioError::Format("HUFF dict1 offset overflow".into()))?;
        if dict1_end > huff.len() {
            return Err(EruditioError::Format("HUFF dict1 out of bounds".into()));
        }
        for i in 0..256 {
            let v = read_u32_be(huff, off1 + i * 4);
            let code_len = v & 0x1f;
            let terminal = (v & 0x80) != 0;
            let max_code = if code_len > 0 {
                (((v >> 8) + 1) << (32 - code_len)).wrapping_sub(1)
            } else {
                0
            };
            self.dict1[i] = Dict1Entry {
                code_len,
                terminal,
                max_code,
            };
        }

        // dict2: 64 u32 entries at off2, alternating (mincode, maxcode) pairs for
        // code lengths 1..32.
        let dict2_end = off2
            .checked_add(64 * 4)
            .ok_or_else(|| EruditioError::Format("HUFF dict2 offset overflow".into()))?;
        if dict2_end > huff.len() {
            return Err(EruditioError::Format("HUFF dict2 out of bounds".into()));
        }
        let mut dict2 = [0u32; 64];
        for (i, entry) in dict2.iter_mut().enumerate() {
            *entry = read_u32_be(huff, off2 + i * 4);
        }

        // Build mincode/maxcode lookup indexed by code length (0..=32).
        // Index 0 is unused (code lengths start at 1).
        self.min_code = Vec::with_capacity(33);
        self.max_code = Vec::with_capacity(33);

        // Sentinel for code_len 0.
        self.min_code.push(0);
        self.max_code.push(0);

        for code_len in 1..=32u32 {
            let idx = (code_len as usize - 1) * 2;
            if idx < 64 {
                let min_val = dict2[idx];
                let max_val = dict2[idx + 1];
                self.min_code.push(min_val << (32 - code_len));
                self.max_code
                    .push(((max_val + 1) << (32 - code_len)).wrapping_sub(1));
            } else {
                self.min_code.push(0);
                self.max_code.push(0);
            }
        }

        Ok(())
    }

    /// Parses a CDIC record and appends its phrase entries to the dictionary.
    fn load_cdic(&mut self, cdic: &[u8]) -> Result<()> {
        if cdic.len() < 16 || &cdic[0..8] != b"CDIC\x00\x00\x00\x10" {
            return Err(EruditioError::Format("Invalid CDIC header".into()));
        }

        let phrases = read_u32_be(cdic, 8) as usize;
        let bits = read_u32_be(cdic, 12) as usize;
        // Cap bit shift to prevent panic/overflow from crafted CDIC headers.
        if bits >= usize::BITS as usize {
            return Err(EruditioError::Format("CDIC bits field too large".into()));
        }
        let n = std::cmp::min(
            1usize << bits,
            phrases.saturating_sub(self.dictionary.len()),
        );

        if 16 + n * 2 > cdic.len() {
            return Err(EruditioError::Format(
                "CDIC offset table out of bounds".into(),
            ));
        }

        for i in 0..n {
            let off = read_u16_be(cdic, 16 + i * 2) as usize;
            let blen_offset = 16 + off;
            if blen_offset + 2 > cdic.len() {
                self.dictionary.push(DictEntry::Decompressed(Vec::new()));
                continue;
            }
            let blen = read_u16_be(cdic, blen_offset) as usize;
            let data_len = blen & 0x7FFF;
            let is_terminal = (blen & 0x8000) != 0;
            let data_start = blen_offset + 2;
            let data_end = std::cmp::min(data_start + data_len, cdic.len());
            let slice = cdic[data_start..data_end].to_vec();

            if is_terminal {
                self.dictionary.push(DictEntry::Decompressed(slice));
            } else {
                self.dictionary.push(DictEntry::Compressed(slice));
            }
        }

        Ok(())
    }

    /// Decompresses a single text record.
    pub fn unpack(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        self.unpack_inner(data, 0)
    }

    /// Inner decompression with depth tracking to prevent infinite recursion.
    fn unpack_inner(&mut self, data: &[u8], depth: usize) -> Result<Vec<u8>> {
        if depth > MAX_DEPTH {
            return Err(EruditioError::Compression(
                "HUFF/CDIC recursion depth exceeded".into(),
            ));
        }

        let mut bits_left = (data.len() as i64) * 8;

        let mut pos: usize = 0;
        let mut x = read_u64_be_padded(data, pos);
        let mut n: i32 = 32; // Number of usable bits in the current window.

        // Pre-allocate result: decompressed text is typically ~2x the compressed size.
        let mut result = Vec::with_capacity(data.len().saturating_mul(2).min(MAX_UNPACK_OUTPUT));

        loop {
            if result.len() > MAX_UNPACK_OUTPUT {
                return Err(EruditioError::Compression(
                    "HUFF/CDIC decompressed output exceeds size limit".into(),
                ));
            }
            if n <= 0 {
                pos += 4;
                x = read_u64_be_padded(data, pos);
                if pos >= data.len() {
                    break;
                }
                n += 32;
            }

            let code = ((x >> n as u64) & 0xFFFF_FFFF) as u32;

            let top8 = (code >> 24) as usize;
            let entry = self.dict1[top8];
            let mut code_len = entry.code_len;
            let mut max_code = entry.max_code;

            if !entry.terminal {
                while code_len < 33 && code < self.min_code[code_len as usize] {
                    code_len += 1;
                }
                if code_len > 32 {
                    break;
                }
                max_code = self.max_code[code_len as usize];
            }

            n -= code_len as i32;
            bits_left -= code_len as i64;
            if bits_left < 0 {
                break;
            }

            let shift = 32u32.saturating_sub(code_len);
            let r = (max_code.wrapping_sub(code) >> shift) as usize;

            if r >= self.dictionary.len() {
                return Err(EruditioError::Compression(format!(
                    "HUFF/CDIC dictionary index {} out of bounds (len {})",
                    r,
                    self.dictionary.len()
                )));
            }

            // Resolve the dictionary entry, decompressing if needed.
            match &self.dictionary[r] {
                DictEntry::Decompressed(slice) => {
                    result.extend_from_slice(slice);
                },
                DictEntry::Compressed(_) => {
                    // Take the compressed data out, decompress it, and cache the result.
                    let compressed = match std::mem::replace(
                        &mut self.dictionary[r],
                        DictEntry::Decompressed(Vec::new()),
                    ) {
                        DictEntry::Compressed(data) => data,
                        _ => {
                            return Err(EruditioError::Compression(
                                "HUFF/CDIC unexpected dictionary state".into(),
                            ));
                        },
                    };
                    let decompressed = self.unpack_inner(&compressed, depth + 1)?;
                    self.dictionary[r] = DictEntry::Decompressed(decompressed);
                    if let DictEntry::Decompressed(ref cached) = self.dictionary[r] {
                        result.extend_from_slice(cached);
                    }
                },
            }
        }

        Ok(result)
    }
}

/// Reads a big-endian u64 from `data` at `offset`, zero-padding bytes beyond
/// the end of the slice. This avoids allocating a padded copy of the input.
#[inline]
fn read_u64_be_padded(data: &[u8], offset: usize) -> u64 {
    let remaining = data.len().saturating_sub(offset);
    if remaining >= 8 {
        u64::from_be_bytes(data[offset..offset + 8].try_into().unwrap())
    } else {
        let mut buf = [0u8; 8];
        let n = remaining.min(8);
        buf[..n].copy_from_slice(&data[offset..offset + n]);
        u64::from_be_bytes(buf)
    }
}

#[inline]
fn read_u32_be(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

#[inline]
fn read_u16_be(data: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([data[offset], data[offset + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal HUFF record with a simple identity-mapping scheme.
    fn build_trivial_huff() -> Vec<u8> {
        // HUFF header: magic (8) + off1 (4) + off2 (4) = 16 bytes minimum
        // dict1 at offset 24 (after header): 256 * 4 = 1024 bytes
        // dict2 at offset 24 + 1024 = 1048: 64 * 4 = 256 bytes
        // Total: 24 + 1024 + 256 = 1304 bytes

        let off1: u32 = 24;
        let off2: u32 = 24 + 256 * 4;
        let mut huff = vec![0u8; off2 as usize + 64 * 4];

        // Magic
        huff[0..8].copy_from_slice(b"HUFF\x00\x00\x00\x18");
        huff[8..12].copy_from_slice(&off1.to_be_bytes());
        huff[12..16].copy_from_slice(&off2.to_be_bytes());

        // dict1: 256 entries. Each byte value maps to code_len=8, terminal=true.
        // packed value: (maxcode_prefix << 8) | 0x80 | code_len
        // For terminal entries with code_len=8: maxcode = ((v>>8)+1) << 24 - 1
        // We want entry i to have maxcode = ((i+1) << 24) - 1
        // So v>>8 = i, meaning packed = (i << 8) | 0x80 | 8
        for i in 0..256u32 {
            let packed = (i << 8) | 0x80 | 8;
            let offset = off1 as usize + i as usize * 4;
            huff[offset..offset + 4].copy_from_slice(&packed.to_be_bytes());
        }

        // dict2: 64 entries (32 pairs of mincode/maxcode).
        // For code_len=8: mincode[8]=0, maxcode[8]=255
        // Pair index for code_len 8 is at indices 14,15 (0-indexed: (8-1)*2=14)
        let pair_offset = off2 as usize + 14 * 4;
        // mincode = 0
        huff[pair_offset..pair_offset + 4].copy_from_slice(&0u32.to_be_bytes());
        // maxcode = 255
        huff[pair_offset + 4..pair_offset + 8].copy_from_slice(&255u32.to_be_bytes());

        huff
    }

    /// Builds a CDIC record with given phrase entries (all terminal).
    fn build_cdic(phrases: u32, bits: u32, entries: &[&[u8]]) -> Vec<u8> {
        let n = entries.len();
        // Header: 16 bytes
        // Offset table: n * 2 bytes
        // Data: sum of (2 + entry.len()) for each entry
        let offset_table_start = 16usize;
        let _data_start = offset_table_start + n * 2;

        let mut cdic = vec![0u8; 16]; // will extend
        cdic[0..8].copy_from_slice(b"CDIC\x00\x00\x00\x10");
        cdic[8..12].copy_from_slice(&phrases.to_be_bytes());
        cdic[12..16].copy_from_slice(&bits.to_be_bytes());

        // Build offset table and data
        let mut offsets = Vec::new();
        let mut data = Vec::new();
        for entry in entries {
            let off = (n * 2 + data.len()) as u16;
            offsets.push(off);
            let blen = (entry.len() as u16) | 0x8000; // terminal flag
            data.extend_from_slice(&blen.to_be_bytes());
            data.extend_from_slice(entry);
        }

        for off in &offsets {
            cdic.extend_from_slice(&off.to_be_bytes());
        }
        cdic.extend_from_slice(&data);

        cdic
    }

    #[test]
    fn load_huff_validates_magic() {
        let bad = vec![0u8; 100];
        let mut reader = HuffCdicReader {
            dict1: [Dict1Entry {
                code_len: 0,
                terminal: false,
                max_code: 0,
            }; 256],
            min_code: Vec::new(),
            max_code: Vec::new(),
            dictionary: Vec::new(),
        };
        assert!(reader.load_huff(&bad).is_err());
    }

    #[test]
    fn load_cdic_validates_magic() {
        let bad = vec![0u8; 100];
        let mut reader = HuffCdicReader {
            dict1: [Dict1Entry {
                code_len: 0,
                terminal: false,
                max_code: 0,
            }; 256],
            min_code: Vec::new(),
            max_code: Vec::new(),
            dictionary: Vec::new(),
        };
        assert!(reader.load_cdic(&bad).is_err());
    }

    #[test]
    fn cdic_entries_parsed_correctly() {
        let entries: Vec<&[u8]> = vec![b"hello", b"world"];
        let cdic = build_cdic(2, 8, &entries);

        let huff = build_trivial_huff();
        let cdic_ref: &[u8] = &cdic;
        let reader = HuffCdicReader::new(&huff, &[cdic_ref]).unwrap();

        assert_eq!(reader.dictionary.len(), 2);
        match &reader.dictionary[0] {
            DictEntry::Decompressed(d) => assert_eq!(d, b"hello"),
            _ => panic!("Expected decompressed entry"),
        }
        match &reader.dictionary[1] {
            DictEntry::Decompressed(d) => assert_eq!(d, b"world"),
            _ => panic!("Expected decompressed entry"),
        }
    }

    #[test]
    fn empty_data_unpacks_to_empty() {
        let entries: Vec<&[u8]> = vec![b"a"];
        let cdic = build_cdic(1, 8, &entries);
        let huff = build_trivial_huff();
        let cdic_ref: &[u8] = &cdic;
        let mut reader = HuffCdicReader::new(&huff, &[cdic_ref]).unwrap();

        let result = reader.unpack(&[]).unwrap();
        assert!(result.is_empty());
    }
}
