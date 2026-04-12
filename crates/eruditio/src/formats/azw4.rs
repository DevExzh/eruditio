//! AZW4 (Amazon Print Replica) reader.
//!
//! AZW4 files are PDF documents wrapped in a PDB/MOBI container.
//! The reader extracts the embedded PDF and delegates to `PdfReader`.

use crate::domain::{Book, FormatReader};
use crate::error::{EruditioError, Result};
use crate::formats::common::MAX_INPUT_SIZE;
use crate::formats::common::palm_db::PdbFile;
use crate::formats::pdf::PdfReader;
use std::io::{Cursor, Read};

/// AZW4 format reader.
///
/// Extracts the embedded PDF from the PDB container and delegates
/// to [`PdfReader`] for actual content extraction.
#[derive(Default)]
pub struct Azw4Reader;

impl Azw4Reader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for Azw4Reader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut data = Vec::new();
        (&mut *reader).take(MAX_INPUT_SIZE).read_to_end(&mut data)?;

        // Extract the PDF from the PDB container.
        let pdf_bytes = extract_pdf(&data)?;

        // Delegate to PdfReader.
        let mut cursor = Cursor::new(pdf_bytes);
        PdfReader::new().read_book(&mut cursor)
    }
}

/// Extracts the embedded PDF from an AZW4 PDB file.
///
/// AZW4 stores the PDF as raw bytes spread across PDB records.
/// We search for the `%PDF` header and `%%EOF` trailer to locate
/// the PDF byte stream, matching calibre's approach.
fn extract_pdf(data: &[u8]) -> Result<Vec<u8>> {
    // Validate this is a PDB file.
    let _pdb = PdbFile::parse(data.to_vec())?;

    // Search for the PDF signature in the raw data.
    let pdf_start = find_subsequence(data, b"%PDF")
        .ok_or_else(|| EruditioError::Format("No embedded PDF found in AZW4 file".into()))?;

    // Find the last %%EOF marker (PDF trailer).
    let pdf_end = rfind_subsequence(data, b"%%EOF")
        .map(|pos| pos + b"%%EOF".len())
        .unwrap_or(data.len());

    if pdf_end <= pdf_start {
        return Err(EruditioError::Format(
            "Invalid PDF boundaries in AZW4 file".into(),
        ));
    }

    Ok(data[pdf_start..pdf_end].to_vec())
}

/// Finds the first occurrence of `needle` in `haystack`.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Finds the last occurrence of `needle` in `haystack`.
fn rfind_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::common::palm_db::{build_pdb_header, write_u16_be, write_u32_be};

    /// Builds a synthetic AZW4 file with an embedded PDF stub.
    fn build_test_azw4(pdf_content: &[u8]) -> Vec<u8> {
        // Build a minimal MOBI-like record 0 with the PDF embedded in record 1.
        let mut record0 = vec![0u8; 16];
        write_u16_be(&mut record0, 0, 1); // compression = none
        write_u32_be(&mut record0, 4, pdf_content.len() as u32);
        write_u16_be(&mut record0, 8, 1); // 1 text record

        let total_records = 2;
        let header_size = 78 + total_records * 8 + 2;

        let mut offsets = Vec::with_capacity(total_records);
        let mut pos = header_size as u32;
        offsets.push(pos);
        pos += record0.len() as u32;
        offsets.push(pos);

        let mut data = build_pdb_header(
            "Test AZW4",
            b"\x00\x00\x00\x00",
            b"\x00\x00\x00\x00",
            total_records as u16,
            &offsets,
        );
        data.extend_from_slice(&record0);
        data.extend_from_slice(pdf_content);

        data
    }

    #[test]
    fn extracts_pdf_from_azw4() {
        let pdf_stub = b"%PDF-1.4\n1 0 obj\n<<>>\nendobj\n%%EOF";
        let azw4_data = build_test_azw4(pdf_stub);

        let extracted = extract_pdf(&azw4_data).unwrap();
        assert!(extracted.starts_with(b"%PDF"));
        assert!(extracted.ends_with(b"%%EOF"));
    }

    #[test]
    fn rejects_azw4_without_pdf() {
        let no_pdf = b"This is not a PDF at all";
        let azw4_data = build_test_azw4(no_pdf);

        let result = extract_pdf(&azw4_data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No embedded PDF"));
    }

    #[test]
    fn find_subsequence_works() {
        assert_eq!(find_subsequence(b"hello world", b"world"), Some(6));
        assert_eq!(find_subsequence(b"hello", b"xyz"), None);
    }

    #[test]
    fn rfind_subsequence_works() {
        let data = b"%%EOF stuff %%EOF";
        assert_eq!(rfind_subsequence(data, b"%%EOF"), Some(12));
    }
}
