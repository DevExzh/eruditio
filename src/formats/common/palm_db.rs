//! PalmDB (PDB) container format parsing.
//!
//! The PDB format is the foundation for MOBI, PRC, eReader, Plucker,
//! and other Palm-era ebook formats. It consists of a 78-byte header
//! followed by a record offset table and the records themselves.

use crate::error::{EruditioError, Result};

/// Size of the fixed PDB file header.
const PDB_HEADER_SIZE: usize = 78;
/// Size of each record entry in the offset table.
const RECORD_ENTRY_SIZE: usize = 8;

/// Well-known PDB type+creator identities (8 bytes at header offsets 60-67).
pub(crate) const IDENTITY_BOOKMOBI: &[u8; 8] = b"BOOKMOBI";
pub(crate) const IDENTITY_TEXTREAD: &[u8; 8] = b"TEXtREAd";

/// Parsed PDB file header.
#[derive(Debug, Clone)]
pub struct PdbHeader {
    /// Database name (up to 31 characters, from the 32-byte null-padded field).
    pub name: String,
    /// Attribute flags.
    pub attributes: u16,
    /// File version.
    pub version: u16,
    /// Creation timestamp (seconds since Palm epoch: 1904-01-01).
    pub creation_date: u32,
    /// Modification timestamp.
    pub modification_date: u32,
    /// Last backup timestamp.
    pub backup_date: u32,
    /// Modification number.
    pub modification_number: u32,
    /// Offset to application info block (0 if none).
    pub app_info_offset: u32,
    /// Offset to sort info block (0 if none).
    pub sort_info_offset: u32,
    /// 4-byte type field (e.g., `BOOK`).
    pub db_type: [u8; 4],
    /// 4-byte creator field (e.g., `MOBI`).
    pub creator: [u8; 4],
    /// Unique ID seed.
    pub unique_id_seed: u32,
    /// Next record list ID.
    pub next_record_list_id: u32,
    /// Number of records.
    pub num_records: u16,
}

impl PdbHeader {
    /// Returns the 8-byte identity (type + creator concatenated).
    pub fn identity(&self) -> [u8; 8] {
        let mut id = [0u8; 8];
        id[..4].copy_from_slice(&self.db_type);
        id[4..].copy_from_slice(&self.creator);
        id
    }

    /// Returns `true` if this is a MOBI/AZW file (`BOOKMOBI` identity).
    pub fn is_mobi(&self) -> bool {
        &self.identity() == IDENTITY_BOOKMOBI
    }

    /// Returns `true` if this is a PalmDOC file (`TEXtREAd` identity).
    pub fn is_palmdoc(&self) -> bool {
        &self.identity() == IDENTITY_TEXTREAD
    }
}

/// A single record entry from the PDB offset table.
#[derive(Debug, Clone, Copy)]
pub struct PdbRecordEntry {
    /// Absolute byte offset of this record in the file.
    pub offset: u32,
    /// Record attribute flags.
    pub flags: u8,
    /// 24-bit unique ID.
    pub unique_id: u32,
}

/// A fully parsed PDB container: header + record table + raw data.
#[derive(Debug, Clone)]
pub struct PdbFile {
    pub header: PdbHeader,
    pub record_entries: Vec<PdbRecordEntry>,
    /// The raw file data (kept for extracting record contents by offset).
    raw: Vec<u8>,
}

impl PdbFile {
    /// Parses a PDB file from raw bytes.
    pub fn parse(data: Vec<u8>) -> Result<Self> {
        if data.len() < PDB_HEADER_SIZE {
            return Err(EruditioError::Format(
                "PDB data too short for header".into(),
            ));
        }

        let header = parse_pdb_header(&data)?;

        let num = header.num_records as usize;
        let table_end = PDB_HEADER_SIZE + num * RECORD_ENTRY_SIZE;
        if data.len() < table_end {
            return Err(EruditioError::Format(
                "PDB data too short for record table".into(),
            ));
        }

        let mut record_entries = Vec::with_capacity(num);
        for i in 0..num {
            let base = PDB_HEADER_SIZE + i * RECORD_ENTRY_SIZE;
            let offset = read_u32_be(&data, base);
            let flags = data[base + 4];
            let unique_id = ((data[base + 5] as u32) << 16)
                | ((data[base + 6] as u32) << 8)
                | (data[base + 7] as u32);
            record_entries.push(PdbRecordEntry {
                offset,
                flags,
                unique_id,
            });
        }

        Ok(Self {
            header,
            record_entries,
            raw: data,
        })
    }

    /// Returns the number of records.
    pub fn record_count(&self) -> usize {
        self.record_entries.len()
    }

    /// Extracts the raw bytes for a record by index.
    ///
    /// The record spans from its offset to the start of the next record
    /// (or end of file for the last record).
    pub fn record_data(&self, index: usize) -> Result<&[u8]> {
        if index >= self.record_entries.len() {
            return Err(EruditioError::Format(format!(
                "Record index {} out of range ({})",
                index,
                self.record_entries.len()
            )));
        }

        let start = self.record_entries[index].offset as usize;
        let end = if index + 1 < self.record_entries.len() {
            self.record_entries[index + 1].offset as usize
        } else {
            self.raw.len()
        };

        if start > self.raw.len() || end > self.raw.len() || start > end {
            return Err(EruditioError::Format(format!(
                "Invalid record bounds: {}..{} (file size: {})",
                start,
                end,
                self.raw.len()
            )));
        }

        Ok(&self.raw[start..end])
    }

    /// Returns the full raw data of the file.
    pub fn raw_data(&self) -> &[u8] {
        &self.raw
    }
}

/// Parses the 78-byte PDB header from raw data.
fn parse_pdb_header(data: &[u8]) -> Result<PdbHeader> {
    // Name: 32 bytes, null-terminated.
    let name_bytes = &data[0..32];
    let name_end = name_bytes.iter().position(|&b| b == 0).unwrap_or(32);
    let name = String::from_utf8_lossy(&name_bytes[..name_end]).into_owned();

    let mut db_type = [0u8; 4];
    db_type.copy_from_slice(&data[60..64]);

    let mut creator = [0u8; 4];
    creator.copy_from_slice(&data[64..68]);

    Ok(PdbHeader {
        name,
        attributes: read_u16_be(data, 32),
        version: read_u16_be(data, 34),
        creation_date: read_u32_be(data, 36),
        modification_date: read_u32_be(data, 40),
        backup_date: read_u32_be(data, 44),
        modification_number: read_u32_be(data, 48),
        app_info_offset: read_u32_be(data, 52),
        sort_info_offset: read_u32_be(data, 56),
        db_type,
        creator,
        unique_id_seed: read_u32_be(data, 68),
        next_record_list_id: read_u32_be(data, 72),
        num_records: read_u16_be(data, 76),
    })
}

/// Reads a big-endian u16 from a byte slice at the given offset.
#[inline]
pub fn read_u16_be(data: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([data[offset], data[offset + 1]])
}

/// Reads a big-endian u32 from a byte slice at the given offset.
#[inline]
pub fn read_u32_be(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

/// Writes a PDB file header and record table to a byte buffer.
///
/// Returns the buffer. Callers append record data and then fix up the
/// record offsets once all data sizes are known.
pub fn build_pdb_header(
    name: &str,
    db_type: &[u8; 4],
    creator: &[u8; 4],
    num_records: u16,
    record_offsets: &[u32],
) -> Vec<u8> {
    let table_size = num_records as usize * RECORD_ENTRY_SIZE;
    let total = PDB_HEADER_SIZE + table_size + 2; // +2 for gap padding
    let mut buf = vec![0u8; total];

    // Name (32 bytes, null-padded).
    let name_bytes = name.as_bytes();
    let copy_len = name_bytes.len().min(31);
    buf[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
    // rest is already zero

    // Attributes, version (leave as 0).
    // Dates (leave as 0 for now).

    // Type and creator.
    buf[60..64].copy_from_slice(db_type);
    buf[64..68].copy_from_slice(creator);

    // Unique ID seed = num_records.
    write_u32_be(&mut buf, 68, num_records as u32);

    // Next record list ID = 0 (already zero).

    // Number of records.
    write_u16_be(&mut buf, 76, num_records);

    // Record offset table.
    for (i, &offset) in record_offsets.iter().enumerate() {
        let base = PDB_HEADER_SIZE + i * RECORD_ENTRY_SIZE;
        write_u32_be(&mut buf, base, offset);
        // flags = 0, unique_id = i
        buf[base + 5] = ((i >> 16) & 0xFF) as u8;
        buf[base + 6] = ((i >> 8) & 0xFF) as u8;
        buf[base + 7] = (i & 0xFF) as u8;
    }

    // 2-byte gap padding (already zero).

    buf
}

/// Writes a big-endian u16 to a byte slice at the given offset.
#[inline]
pub fn write_u16_be(buf: &mut [u8], offset: usize, value: u16) {
    let bytes = value.to_be_bytes();
    buf[offset] = bytes[0];
    buf[offset + 1] = bytes[1];
}

/// Writes a big-endian u32 to a byte slice at the given offset.
#[inline]
pub fn write_u32_be(buf: &mut [u8], offset: usize, value: u32) {
    let bytes = value.to_be_bytes();
    buf[offset] = bytes[0];
    buf[offset + 1] = bytes[1];
    buf[offset + 2] = bytes[2];
    buf[offset + 3] = bytes[3];
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal valid PDB file with the given identity and records.
    fn build_test_pdb(db_type: &[u8; 4], creator: &[u8; 4], records: &[&[u8]]) -> Vec<u8> {
        let num = records.len() as u16;
        let header_size = PDB_HEADER_SIZE + (num as usize) * RECORD_ENTRY_SIZE + 2;

        // Calculate record offsets.
        let mut offsets = Vec::with_capacity(records.len());
        let mut pos = header_size as u32;
        for rec in records {
            offsets.push(pos);
            pos += rec.len() as u32;
        }

        let mut data = build_pdb_header("TestDB", db_type, creator, num, &offsets);

        // Append records.
        for rec in records {
            data.extend_from_slice(rec);
        }

        data
    }

    #[test]
    fn parse_pdb_header_basic() {
        let data = build_test_pdb(b"BOOK", b"MOBI", &[b"record0", b"record1"]);
        let pdb = PdbFile::parse(data).unwrap();

        assert_eq!(pdb.header.name, "TestDB");
        assert_eq!(&pdb.header.db_type, b"BOOK");
        assert_eq!(&pdb.header.creator, b"MOBI");
        assert_eq!(pdb.header.num_records, 2);
        assert!(pdb.header.is_mobi());
    }

    #[test]
    fn record_data_extraction() {
        let data = build_test_pdb(b"BOOK", b"MOBI", &[b"hello", b"world"]);
        let pdb = PdbFile::parse(data).unwrap();

        assert_eq!(pdb.record_count(), 2);
        assert_eq!(pdb.record_data(0).unwrap(), b"hello");
        assert_eq!(pdb.record_data(1).unwrap(), b"world");
    }

    #[test]
    fn record_index_out_of_range() {
        let data = build_test_pdb(b"TEXt", b"REAd", &[b"only"]);
        let pdb = PdbFile::parse(data).unwrap();

        assert!(pdb.record_data(1).is_err());
    }

    #[test]
    fn identity_detection() {
        let data = build_test_pdb(b"TEXt", b"REAd", &[b"x"]);
        let pdb = PdbFile::parse(data).unwrap();

        assert!(pdb.header.is_palmdoc());
        assert!(!pdb.header.is_mobi());
    }

    #[test]
    fn too_short_data_rejected() {
        let result = PdbFile::parse(vec![0u8; 10]);
        assert!(result.is_err());
    }

    #[test]
    fn read_write_u16_round_trip() {
        let mut buf = [0u8; 4];
        write_u16_be(&mut buf, 1, 0xABCD);
        assert_eq!(read_u16_be(&buf, 1), 0xABCD);
    }

    #[test]
    fn read_write_u32_round_trip() {
        let mut buf = [0u8; 8];
        write_u32_be(&mut buf, 2, 0xDEAD_BEEF);
        assert_eq!(read_u32_be(&buf, 2), 0xDEAD_BEEF);
    }
}
