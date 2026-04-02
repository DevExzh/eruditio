//! EXTH (Extended Header) parsing for MOBI files.
//!
//! The EXTH header is an optional block within Record 0, immediately after
//! the MOBI header. It contains a variable number of typed metadata records
//! (author, publisher, ISBN, description, cover offset, etc.).

use crate::error::{EruditioError, Result};
use crate::formats::common::palm_db::{read_u32_be, write_u32_be};

// --- Well-known EXTH record type codes ---

pub const EXTH_AUTHOR: u32 = 100;
pub const EXTH_PUBLISHER: u32 = 101;
pub const EXTH_DESCRIPTION: u32 = 103;
pub const EXTH_ISBN: u32 = 104;
pub const EXTH_SUBJECT: u32 = 105;
pub const EXTH_PUBLISHED_DATE: u32 = 106;
pub const EXTH_RIGHTS: u32 = 109;
pub const EXTH_ASIN: u32 = 113;
pub const EXTH_ADULT: u32 = 117;
pub const EXTH_KF8_BOUNDARY: u32 = 121;
pub const EXTH_COVER_OFFSET: u32 = 201;
pub const EXTH_THUMB_OFFSET: u32 = 202;
pub const EXTH_CREATOR_SOFTWARE: u32 = 204;
pub const EXTH_UPDATED_TITLE: u32 = 503;
pub const EXTH_LANGUAGE: u32 = 524;
pub const EXTH_CDE_TYPE: u32 = 501;

/// A single EXTH record.
#[derive(Debug, Clone)]
pub struct ExthRecord {
    /// Record type code.
    pub record_type: u32,
    /// Raw record data (interpretation depends on type).
    pub data: Vec<u8>,
}

impl ExthRecord {
    /// Returns the record data interpreted as a UTF-8 string.
    pub fn as_string(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }

    /// Returns the record data interpreted as a big-endian u32.
    pub fn as_u32(&self) -> Option<u32> {
        if self.data.len() >= 4 {
            Some(read_u32_be(&self.data, 0))
        } else {
            None
        }
    }
}

/// Parsed EXTH header containing all metadata records.
#[derive(Debug, Clone, Default)]
pub struct ExthHeader {
    pub records: Vec<ExthRecord>,
}

impl ExthHeader {
    /// Parses the EXTH header from a byte slice starting at the EXTH magic.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 12 {
            return Err(EruditioError::Format(
                "EXTH data too short for header".into(),
            ));
        }

        // Verify magic.
        if &data[0..4] != b"EXTH" {
            return Err(EruditioError::Format(
                "Missing EXTH magic identifier".into(),
            ));
        }

        let _total_length = read_u32_be(data, 4);
        let num_records = read_u32_be(data, 8);

        let mut records = Vec::with_capacity(num_records as usize);
        let mut pos = 12;

        for _ in 0..num_records {
            if pos + 8 > data.len() {
                break; // Truncated — stop gracefully.
            }

            let record_type = read_u32_be(data, pos);
            let record_length = read_u32_be(data, pos + 4) as usize;

            if record_length < 8 || pos + record_length > data.len() {
                break; // Invalid or truncated record.
            }

            let record_data = data[pos + 8..pos + record_length].to_vec();
            records.push(ExthRecord {
                record_type,
                data: record_data,
            });

            pos += record_length;
        }

        Ok(Self { records })
    }

    /// Returns the first record of the given type as a string, if present.
    pub fn get_string(&self, record_type: u32) -> Option<String> {
        self.records
            .iter()
            .find(|r| r.record_type == record_type)
            .map(|r| r.as_string())
    }

    /// Returns all records of the given type as strings.
    pub fn get_all_strings(&self, record_type: u32) -> Vec<String> {
        self.records
            .iter()
            .filter(|r| r.record_type == record_type)
            .map(|r| r.as_string())
            .collect()
    }

    /// Returns the first record of the given type as a u32, if present.
    pub fn get_u32(&self, record_type: u32) -> Option<u32> {
        self.records
            .iter()
            .find(|r| r.record_type == record_type)
            .and_then(|r| r.as_u32())
    }

    /// Returns the KF8 boundary section index, if present.
    pub fn kf8_boundary(&self) -> Option<u32> {
        self.get_u32(EXTH_KF8_BOUNDARY)
    }

    /// Returns the cover image offset (relative to first image record).
    pub fn cover_offset(&self) -> Option<u32> {
        self.get_u32(EXTH_COVER_OFFSET)
    }
}

/// Builds a serialized EXTH header block from a list of (type, data) pairs.
pub fn build_exth(records: &[(u32, &[u8])]) -> Vec<u8> {
    if records.is_empty() {
        return Vec::new();
    }

    let mut record_bytes = Vec::new();
    for &(record_type, data) in records {
        let length = (data.len() + 8) as u32;
        let mut entry = vec![0u8; 8 + data.len()];
        write_u32_be(&mut entry, 0, record_type);
        write_u32_be(&mut entry, 4, length);
        entry[8..].copy_from_slice(data);
        record_bytes.extend_from_slice(&entry);
    }

    let total_length = 12 + record_bytes.len();
    // Pad to 4-byte alignment.
    let padding = (4 - (total_length % 4)) % 4;

    let mut exth = Vec::with_capacity(total_length + padding);
    exth.extend_from_slice(b"EXTH");

    let mut len_bytes = [0u8; 4];
    write_u32_be(&mut len_bytes, 0, (total_length + padding) as u32);
    exth.extend_from_slice(&len_bytes);

    let mut count_bytes = [0u8; 4];
    write_u32_be(&mut count_bytes, 0, records.len() as u32);
    exth.extend_from_slice(&count_bytes);

    exth.extend_from_slice(&record_bytes);
    exth.extend(std::iter::repeat_n(0u8, padding));

    exth
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_test_exth(records: &[(u32, &[u8])]) -> Vec<u8> {
        build_exth(records)
    }

    #[test]
    fn parse_exth_single_string_record() {
        let exth = build_test_exth(&[(EXTH_AUTHOR, b"Jane Doe")]);
        let parsed = ExthHeader::parse(&exth).unwrap();

        assert_eq!(parsed.records.len(), 1);
        assert_eq!(parsed.get_string(EXTH_AUTHOR).unwrap(), "Jane Doe");
    }

    #[test]
    fn parse_exth_multiple_records() {
        let exth = build_test_exth(&[
            (EXTH_AUTHOR, b"Author One"),
            (EXTH_AUTHOR, b"Author Two"),
            (EXTH_PUBLISHER, b"Publisher"),
            (EXTH_ISBN, b"978-0-123456-78-9"),
        ]);
        let parsed = ExthHeader::parse(&exth).unwrap();

        assert_eq!(parsed.records.len(), 4);

        let authors = parsed.get_all_strings(EXTH_AUTHOR);
        assert_eq!(authors, vec!["Author One", "Author Two"]);
        assert_eq!(parsed.get_string(EXTH_PUBLISHER).unwrap(), "Publisher");
        assert_eq!(parsed.get_string(EXTH_ISBN).unwrap(), "978-0-123456-78-9");
    }

    #[test]
    fn parse_exth_u32_record() {
        let offset_bytes = 42u32.to_be_bytes();
        let exth = build_test_exth(&[(EXTH_COVER_OFFSET, &offset_bytes)]);
        let parsed = ExthHeader::parse(&exth).unwrap();

        assert_eq!(parsed.cover_offset(), Some(42));
    }

    #[test]
    fn parse_exth_missing_record_returns_none() {
        let exth = build_test_exth(&[(EXTH_AUTHOR, b"Name")]);
        let parsed = ExthHeader::parse(&exth).unwrap();

        assert!(parsed.get_string(EXTH_ISBN).is_none());
        assert!(parsed.cover_offset().is_none());
    }

    #[test]
    fn parse_exth_missing_magic_rejected() {
        let result = ExthHeader::parse(b"NOTExxxxyyyyzzzz");
        assert!(result.is_err());
    }

    #[test]
    fn build_exth_round_trip() {
        let records = vec![
            (EXTH_AUTHOR, "Author".as_bytes()),
            (EXTH_DESCRIPTION, "A great book".as_bytes()),
        ];
        let built = build_exth(&records);
        let parsed = ExthHeader::parse(&built).unwrap();

        assert_eq!(parsed.records.len(), 2);
        assert_eq!(parsed.get_string(EXTH_AUTHOR).unwrap(), "Author");
        assert_eq!(
            parsed.get_string(EXTH_DESCRIPTION).unwrap(),
            "A great book"
        );
    }

    #[test]
    fn build_exth_empty() {
        let built = build_exth(&[]);
        assert!(built.is_empty());
    }

    #[test]
    fn kf8_boundary_parsed() {
        let boundary_bytes = 500u32.to_be_bytes();
        let exth = build_test_exth(&[(EXTH_KF8_BOUNDARY, &boundary_bytes)]);
        let parsed = ExthHeader::parse(&exth).unwrap();

        assert_eq!(parsed.kf8_boundary(), Some(500));
    }
}
