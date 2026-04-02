//! PalmDOC and MOBI header parsing for Record 0.
//!
//! Record 0 of a MOBI file contains three concatenated structures:
//! 1. PalmDOC header (16 bytes)
//! 2. MOBI header (variable, typically 228 or 264 bytes)
//! 3. EXTH header (optional, indicated by EXTH flag)
//! 4. Full title string (at an offset specified in the MOBI header)
#![allow(dead_code)]

use crate::error::{EruditioError, Result};
use crate::formats::common::palm_db::{read_u16_be, read_u32_be};

/// Sentinel value for "not present" index fields.
pub(crate) const NULL_INDEX: u32 = 0xFFFF_FFFF;

/// Maximum uncompressed text record size.
pub(crate) const RECORD_SIZE: usize = 4096;

// --- Compression types ---

/// No compression.
pub(crate) const COMPRESSION_NONE: u16 = 1;
/// PalmDoc LZ77 compression.
pub(crate) const COMPRESSION_PALMDOC: u16 = 2;
/// Huff/CDIC compression.
pub(crate) const COMPRESSION_HUFFCDIC: u16 = 17480; // 0x4448 = 'DH'

// --- Encryption types ---

/// No encryption.
pub(crate) const ENCRYPTION_NONE: u16 = 0;

// --- Text encodings ---

/// Windows-1252 encoding.
pub(crate) const ENCODING_CP1252: u32 = 1252;
/// UTF-8 encoding.
pub(crate) const ENCODING_UTF8: u32 = 65001;

/// Parsed PalmDOC header (first 16 bytes of Record 0).
#[derive(Debug, Clone)]
pub(crate) struct PalmDocHeader {
    /// Compression type (1=none, 2=PalmDoc, 17480=HUFF/CDIC).
    pub compression: u16,
    /// Total uncompressed text length in bytes.
    pub text_length: u32,
    /// Number of PDB text records.
    pub text_record_count: u16,
    /// Maximum record size (always 4096).
    pub record_size: u16,
    /// Encryption type (0=none, 1=old, 2=mobipocket).
    pub encryption: u16,
}

impl PalmDocHeader {
    /// Parses the PalmDOC header from the first 16 bytes of Record 0.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 16 {
            return Err(EruditioError::Format(
                "Record 0 too short for PalmDOC header".into(),
            ));
        }

        Ok(Self {
            compression: read_u16_be(data, 0),
            text_length: read_u32_be(data, 4),
            text_record_count: read_u16_be(data, 8),
            record_size: read_u16_be(data, 10),
            encryption: read_u16_be(data, 12),
        })
    }

    pub fn is_encrypted(&self) -> bool {
        self.encryption != ENCRYPTION_NONE
    }
}

/// Parsed MOBI header (starts at offset 16 in Record 0).
#[derive(Debug, Clone)]
pub(crate) struct MobiHeader {
    /// Header length in bytes.
    pub header_length: u32,
    /// MOBI type (2=book, 3=PalmDOC, etc.).
    pub mobi_type: u32,
    /// Text encoding (1252=cp1252, 65001=utf-8).
    pub encoding: u32,
    /// Unique ID.
    pub unique_id: u32,
    /// File version (6=Mobi6, 8=KF8/AZW3).
    pub file_version: u32,
    /// First non-book record index.
    pub first_non_book_record: u32,
    /// Offset of the full title within Record 0.
    pub full_name_offset: u32,
    /// Length of the full title.
    pub full_name_length: u32,
    /// Locale / language code.
    pub locale: u32,
    /// Minimum reader version required.
    pub min_version: u32,
    /// Index of the first image record.
    pub first_image_index: u32,
    /// Huffman record offset (for HUFF/CDIC).
    pub huffman_record_offset: u32,
    /// Huffman record count.
    pub huffman_record_count: u32,
    /// EXTH flags (bit 6 = EXTH header present).
    pub exth_flags: u32,
    /// DRM offset (NULL_INDEX if none).
    pub drm_offset: u32,
    /// DRM count.
    pub drm_count: u32,
    /// First content record (Mobi6) or FDST index high (KF8).
    pub first_content_record: u16,
    /// Last content record (Mobi6) or FDST index low (KF8).
    pub last_content_record: u16,
    /// Extra data flags (bit 0=multibyte, bit 1=TBS, bit 2=uncrossable).
    pub extra_data_flags: u32,
    /// NCX index record (NULL_INDEX if none).
    pub ncx_index: u32,
    // --- KF8 fields (only present if header_length >= 248) ---
    /// Fragment (chunk) index.
    pub fragment_index: Option<u32>,
    /// Skeleton index.
    pub skeleton_index: Option<u32>,

    /// The full title extracted from Record 0.
    pub full_title: String,
}

impl MobiHeader {
    /// Parses the MOBI header from Record 0 data (starting at offset 16).
    ///
    /// `record0` is the entire Record 0 data (including the PalmDOC header prefix).
    pub fn parse(record0: &[u8]) -> Result<Self> {
        if record0.len() < 20 {
            return Err(EruditioError::Format(
                "Record 0 too short for MOBI header".into(),
            ));
        }

        // Verify MOBI magic at offset 16.
        if &record0[16..20] != b"MOBI" {
            return Err(EruditioError::Format(
                "Missing MOBI magic identifier".into(),
            ));
        }

        let header_length = read_u32_be(record0, 20);

        // Minimum MOBI header is about 116 bytes (offset 16 + 116 = 132).
        // But most fields we need are within the first 244 bytes of Record 0.
        let mobi_end = 16 + header_length as usize;
        if record0.len() < mobi_end.min(132) {
            return Err(EruditioError::Format(
                "Record 0 too short for declared MOBI header length".into(),
            ));
        }

        let full_name_offset = read_u32_safe(record0, 84);
        let full_name_length = read_u32_safe(record0, 88);

        // Extract full title.
        let full_title = extract_full_title(record0, full_name_offset, full_name_length);

        // KF8 fields (only if header is long enough).
        let fragment_index = if header_length >= 232 {
            Some(read_u32_safe(record0, 248))
        } else {
            None
        };
        let skeleton_index = if header_length >= 236 {
            Some(read_u32_safe(record0, 252))
        } else {
            None
        };

        Ok(Self {
            header_length,
            mobi_type: read_u32_be(record0, 24),
            encoding: read_u32_be(record0, 28),
            unique_id: read_u32_be(record0, 32),
            file_version: read_u32_be(record0, 36),
            first_non_book_record: read_u32_safe(record0, 80),
            full_name_offset,
            full_name_length,
            locale: read_u32_safe(record0, 92),
            min_version: read_u32_safe(record0, 104),
            first_image_index: read_u32_safe(record0, 108),
            huffman_record_offset: read_u32_safe(record0, 112),
            huffman_record_count: read_u32_safe(record0, 116),
            exth_flags: read_u32_safe(record0, 128),
            drm_offset: read_u32_safe(record0, 168),
            drm_count: read_u32_safe(record0, 172),
            first_content_record: read_u16_safe(record0, 192),
            last_content_record: read_u16_safe(record0, 194),
            extra_data_flags: read_u32_safe(record0, 240),
            ncx_index: read_u32_safe(record0, 244),
            fragment_index,
            skeleton_index,
            full_title,
        })
    }

    /// Returns `true` if the EXTH header is present.
    pub fn has_exth(&self) -> bool {
        self.exth_flags & 0x40 != 0
    }

    /// Returns `true` if this is a KF8/AZW3 file (version 8).
    pub fn is_kf8(&self) -> bool {
        self.file_version >= 8
    }

    /// Returns `true` if text encoding is UTF-8.
    pub fn is_utf8(&self) -> bool {
        self.encoding == ENCODING_UTF8
    }

    /// Returns the number of extra bytes appended to each text record.
    /// These trailing bytes must be stripped before decompression.
    pub fn trailing_entry_count(&self) -> u32 {
        let flags = self.extra_data_flags;
        // Bits 1-3+ indicate trailing data entries; bit 0 indicates multibyte.
        let mut count = 0;
        let mut f = flags >> 1;
        while f > 0 {
            if f & 1 != 0 {
                count += 1;
            }
            f >>= 1;
        }
        count
    }

    /// Returns `true` if text records have multibyte trailing data.
    pub fn has_multibyte(&self) -> bool {
        self.extra_data_flags & 1 != 0
    }

    /// Returns the byte offset where the EXTH header starts in Record 0.
    pub fn exth_offset(&self) -> usize {
        16 + self.header_length as usize
    }
}

/// Reads a u32 from data, returning 0 if the offset is out of bounds.
fn read_u32_safe(data: &[u8], offset: usize) -> u32 {
    if offset + 4 <= data.len() {
        read_u32_be(data, offset)
    } else {
        0
    }
}

/// Reads a u16 from data, returning 0 if the offset is out of bounds.
fn read_u16_safe(data: &[u8], offset: usize) -> u16 {
    if offset + 2 <= data.len() {
        read_u16_be(data, offset)
    } else {
        0
    }
}

/// Extracts the full book title from Record 0.
fn extract_full_title(record0: &[u8], offset: u32, length: u32) -> String {
    let start = offset as usize;
    let end = match start.checked_add(length as usize) {
        Some(e) => e,
        None => return String::new(), // overflow from crafted header values
    };

    if start < record0.len() && end <= record0.len() && start < end {
        String::from_utf8_lossy(&record0[start..end])
            .trim_end_matches('\0')
            .to_string()
    } else {
        String::new()
    }
}

/// Calculates trailing data size for a text record.
///
/// MOBI text records may have extra bytes appended after the compressed
/// data. These must be stripped before decompression.
pub(crate) fn trailing_data_size(record_data: &[u8], extra_flags: u32) -> usize {
    let mut total = 0;

    // Process trailing entry flags (bits 1+).
    let mut flags = extra_flags >> 1;
    while flags > 0 {
        if flags & 1 != 0 {
            total += trailing_entry_size(record_data, total);
        }
        flags >>= 1;
    }

    // Multibyte trailing data (bit 0).
    if extra_flags & 1 != 0 {
        let effective_len = record_data.len().saturating_sub(total);
        if effective_len > 0 {
            let last_byte = record_data[effective_len - 1];
            let mb_size = (last_byte & 0x03) as usize + 1;
            total += mb_size;
        }
    }

    total
}

/// Computes the size of a single trailing entry (variable-width integer at end).
fn trailing_entry_size(data: &[u8], already_consumed: usize) -> usize {
    let effective_len = data.len() - already_consumed;
    if effective_len == 0 {
        return 0;
    }

    // Variable-width backward integer: read bytes from end,
    // each contributes 7 bits, MSB indicates "more".
    let mut size = 0u32;
    let mut shift = 0;
    for idx in (0..effective_len).rev() {
        let byte = data[idx];
        size |= ((byte & 0x7F) as u32) << shift;
        shift += 7;
        if byte & 0x80 != 0 {
            break;
        }
    }

    size as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::common::palm_db::write_u32_be;

    /// Builds a minimal MOBI Record 0 with the given parameters.
    fn build_record0(
        compression: u16,
        text_length: u32,
        text_records: u16,
        encoding: u32,
        file_version: u32,
        exth_flags: u32,
        title: &str,
    ) -> Vec<u8> {
        let title_bytes = title.as_bytes();
        // PalmDOC (16) + MOBI header (228) + title
        let mobi_header_len: u32 = 228;
        let title_offset = 16 + mobi_header_len;
        let total_size = title_offset as usize + title_bytes.len();
        let mut data = vec![0u8; total_size];

        // PalmDOC header.
        let comp_bytes = compression.to_be_bytes();
        data[0] = comp_bytes[0];
        data[1] = comp_bytes[1];
        write_u32_be(&mut data, 4, text_length);
        let tr_bytes = text_records.to_be_bytes();
        data[8] = tr_bytes[0];
        data[9] = tr_bytes[1];
        data[10] = 0x10; // record size = 4096
        data[11] = 0x00;

        // MOBI magic.
        data[16..20].copy_from_slice(b"MOBI");
        // Header length.
        write_u32_be(&mut data, 20, mobi_header_len);
        // MOBI type = 2 (book).
        write_u32_be(&mut data, 24, 2);
        // Encoding.
        write_u32_be(&mut data, 28, encoding);
        // File version.
        write_u32_be(&mut data, 36, file_version);
        // Full name offset + length.
        write_u32_be(&mut data, 84, title_offset);
        write_u32_be(&mut data, 88, title_bytes.len() as u32);
        // EXTH flags.
        write_u32_be(&mut data, 128, exth_flags);
        // First image index = NULL.
        write_u32_be(&mut data, 108, NULL_INDEX);
        // DRM offset = NULL.
        write_u32_be(&mut data, 168, NULL_INDEX);

        // Title.
        data[title_offset as usize..].copy_from_slice(title_bytes);

        data
    }

    #[test]
    fn palmdoc_header_parse() {
        let record0 = build_record0(2, 50000, 13, ENCODING_UTF8, 6, 0, "Test");
        let pdh = PalmDocHeader::parse(&record0).unwrap();

        assert_eq!(pdh.compression, COMPRESSION_PALMDOC);
        assert_eq!(pdh.text_length, 50000);
        assert_eq!(pdh.text_record_count, 13);
        assert_eq!(pdh.record_size, 4096);
        assert!(!pdh.is_encrypted());
    }

    #[test]
    fn mobi_header_parse() {
        let record0 = build_record0(2, 50000, 13, ENCODING_UTF8, 6, 0x40, "My Book");
        let mh = MobiHeader::parse(&record0).unwrap();

        assert_eq!(mh.header_length, 228);
        assert_eq!(mh.encoding, ENCODING_UTF8);
        assert_eq!(mh.file_version, 6);
        assert!(mh.is_utf8());
        assert!(!mh.is_kf8());
        assert!(mh.has_exth());
        assert_eq!(mh.full_title, "My Book");
    }

    #[test]
    fn mobi_header_no_exth() {
        let record0 = build_record0(1, 1000, 1, ENCODING_CP1252, 6, 0, "Plain");
        let mh = MobiHeader::parse(&record0).unwrap();

        assert!(!mh.has_exth());
        assert!(!mh.is_utf8());
        assert_eq!(mh.full_title, "Plain");
    }

    #[test]
    fn mobi_header_missing_magic_rejected() {
        let mut data = vec![0u8; 256];
        data[16..20].copy_from_slice(b"NOPE");
        let result = MobiHeader::parse(&data);
        assert!(result.is_err());
    }

    #[test]
    fn trailing_data_size_no_extras() {
        let data = vec![0u8; 100];
        assert_eq!(trailing_data_size(&data, 0), 0);
    }

    #[test]
    fn trailing_data_size_multibyte_only() {
        // Last byte indicates 2 multibyte trailing bytes (0x01 & 0x03 = 1, +1 = 2).
        let mut data = vec![b'X'; 20];
        data[19] = 0x01;
        assert_eq!(trailing_data_size(&data, 1), 2);
    }
}
