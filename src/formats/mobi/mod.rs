//! MOBI/AZW/PRC format reader and writer.
//!
//! The MOBI format is built on the PalmDB (PDB) container. A MOBI file
//! contains a PDB header, a Record 0 with PalmDOC + MOBI + EXTH headers,
//! compressed text records, and image records.

pub mod exth;
pub mod header;
pub mod writer;

use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use crate::formats::common::compression::huffcdic::HuffCdicReader;
use crate::formats::common::compression::palmdoc;
use crate::formats::common::palm_db::PdbFile;
use std::io::{Read, Write};

use self::exth::{ExthHeader, EXTH_AUTHOR, EXTH_DESCRIPTION, EXTH_ISBN, EXTH_PUBLISHER,
    EXTH_SUBJECT, EXTH_UPDATED_TITLE, EXTH_LANGUAGE};
use self::header::{
    MobiHeader, PalmDocHeader, COMPRESSION_HUFFCDIC, COMPRESSION_NONE, COMPRESSION_PALMDOC,
    NULL_INDEX,
};

/// Non-text record signatures that should be skipped when extracting images.
const NON_IMAGE_SIGS: &[&[u8]] = &[
    b"FLIS", b"FCIS", b"SRCS", b"RESC", b"BOUN", b"FDST", b"DATP", b"AUDI", b"VIDE",
    b"\xe9\x8e\r\n",
    b"BOUNDARY",
];

/// MOBI format reader.
#[derive(Default)]
pub struct MobiReader;

impl MobiReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for MobiReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).map_err(EruditioError::Io)?;

        let pdb = PdbFile::parse(buffer)?;

        // Verify identity.
        let identity = pdb.header.identity();
        if &identity != b"BOOKMOBI" && &identity != b"TEXtREAd" {
            return Err(EruditioError::Format(format!(
                "Not a MOBI/PalmDOC file (identity: {:?})",
                String::from_utf8_lossy(&identity)
            )));
        }

        if pdb.record_count() == 0 {
            return Err(EruditioError::Format("MOBI file has no records".into()));
        }

        let record0 = pdb.record_data(0)?;

        // Parse headers.
        let palmdoc = PalmDocHeader::parse(record0)?;

        if palmdoc.is_encrypted() {
            return Err(EruditioError::Format(
                "Encrypted MOBI files are not supported".into(),
            ));
        }

        // PalmDOC files (TEXtREAd) may not have a MOBI header.
        let is_palmdoc_only = &identity == b"TEXtREAd" && record0.len() < 20
            || (record0.len() >= 20 && &record0[16..20] != b"MOBI");

        let (mobi_header, exth) = if is_palmdoc_only {
            (None, None)
        } else {
            let mh = MobiHeader::parse(record0)?;
            let ex = if mh.has_exth() {
                let exth_start = mh.exth_offset();
                if exth_start < record0.len() {
                    ExthHeader::parse(&record0[exth_start..]).ok()
                } else {
                    None
                }
            } else {
                None
            };
            (Some(mh), ex)
        };

        // Decompress text records.
        let text = decompress_text(&pdb, &palmdoc, mobi_header.as_ref())?;

        // Build the Book.
        let mut book = Book::new();

        // Metadata from MOBI header + EXTH.
        populate_metadata(&mut book, mobi_header.as_ref(), exth.as_ref());

        // Content: treat the decompressed text as a single HTML chapter.
        // MOBI content is typically HTML with inline formatting.
        let content = if mobi_header.as_ref().is_some_and(|h| h.is_utf8()) {
            String::from_utf8_lossy(&text).into_owned()
        } else {
            // CP-1252 fallback: decode common characters, lossy for others.
            decode_cp1252(&text)
        };

        // Split into chapters by filepos anchors or treat as single chapter.
        let chapters = split_mobi_content(&content);
        for (i, ch) in chapters.iter().enumerate() {
            book.add_chapter(&Chapter {
                title: ch.title.clone(),
                content: ch.content.clone(),
                id: Some(format!("mobi_ch_{}", i)),
            });
        }

        // Extract images.
        extract_images(&pdb, &mut book, mobi_header.as_ref());

        Ok(book)
    }
}

/// MOBI format writer.
///
/// Produces MOBI version 6 files with PalmDoc compression, EXTH metadata,
/// and embedded images.
#[derive(Default)]
pub struct MobiWriter;

impl MobiWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for MobiWriter {
    fn write_book(&self, book: &Book, output: &mut dyn Write) -> Result<()> {
        let data = writer::write_mobi(book)?;
        output.write_all(&data).map_err(EruditioError::Io)
    }
}

// --- Internal helpers ---

/// Decompresses all text records and returns the concatenated raw text.
fn decompress_text(
    pdb: &PdbFile,
    palmdoc: &PalmDocHeader,
    mobi_header: Option<&MobiHeader>,
) -> Result<Vec<u8>> {
    let num_text_records = palmdoc.text_record_count as usize;
    let extra_flags = mobi_header.map(|h| h.extra_data_flags).unwrap_or(0);

    let mut text = Vec::with_capacity(palmdoc.text_length as usize);
    let mut huff_reader: Option<HuffCdicReader> = None;

    for i in 1..=num_text_records {
        if i >= pdb.record_count() {
            break;
        }

        let raw_record = pdb.record_data(i)?;

        // Strip trailing data if present.
        let trailing = header::trailing_data_size(raw_record, extra_flags);
        let record_data = if trailing < raw_record.len() {
            &raw_record[..raw_record.len() - trailing]
        } else {
            raw_record
        };

        match palmdoc.compression {
            COMPRESSION_NONE => {
                text.extend_from_slice(record_data);
            }
            COMPRESSION_PALMDOC => {
                let decompressed = palmdoc::decompress(record_data)?;
                text.extend_from_slice(&decompressed);
            }
            COMPRESSION_HUFFCDIC => {
                // HUFF/CDIC: lazily initialize the decompressor on first use.
                if huff_reader.is_none() {
                    huff_reader = Some(build_huffcdic_reader(pdb, mobi_header)?);
                }
                let reader = huff_reader.as_mut().unwrap();
                let decompressed = reader.unpack(record_data).map_err(|e| {
                    EruditioError::Compression(format!("HUFF/CDIC decompression failed: {}", e))
                })?;
                text.extend_from_slice(&decompressed);
            }
            other => {
                return Err(EruditioError::Format(format!(
                    "Unknown MOBI compression type: {}",
                    other
                )));
            }
        }
    }

    Ok(text)
}

/// Builds a HUFF/CDIC decompressor from the PDB records referenced by the MOBI header.
fn build_huffcdic_reader(
    pdb: &PdbFile,
    mobi_header: Option<&MobiHeader>,
) -> Result<HuffCdicReader> {
    let mh = mobi_header.ok_or_else(|| {
        EruditioError::Format("HUFF/CDIC compression requires a MOBI header".into())
    })?;

    let huff_offset = mh.huffman_record_offset as usize;
    let huff_count = mh.huffman_record_count as usize;

    if huff_count == 0 || huff_offset == 0 || huff_offset >= pdb.record_count() {
        return Err(EruditioError::Format(
            "Invalid HUFF/CDIC record offset or count".into(),
        ));
    }

    let huff_record = pdb.record_data(huff_offset)?;

    let mut cdic_refs: Vec<&[u8]> = Vec::with_capacity(huff_count.saturating_sub(1));
    for i in 1..huff_count {
        let idx = huff_offset + i;
        if idx < pdb.record_count() {
            cdic_refs.push(pdb.record_data(idx)?);
        }
    }

    HuffCdicReader::new(huff_record, &cdic_refs)
}

/// Populates Book metadata from MOBI header and EXTH records.
fn populate_metadata(book: &mut Book, mobi: Option<&MobiHeader>, exth: Option<&ExthHeader>) {
    // Title: prefer EXTH updated title, then MOBI full title.
    if let Some(ex) = exth
        && let Some(title) = ex.get_string(EXTH_UPDATED_TITLE)
        && !title.is_empty()
    {
        book.metadata.title = Some(title);
    }
    if book.metadata.title.is_none()
        && let Some(mh) = mobi
        && !mh.full_title.is_empty()
    {
        book.metadata.title = Some(mh.full_title.clone());
    }

    if let Some(ex) = exth {
        // Authors (may have multiple EXTH 100 records).
        let authors = ex.get_all_strings(EXTH_AUTHOR);
        for author in authors {
            if !author.is_empty() {
                book.metadata.authors.push(author);
            }
        }

        // Publisher.
        if let Some(publisher) = ex.get_string(EXTH_PUBLISHER) {
            book.metadata.publisher = Some(publisher);
        }

        // Description.
        if let Some(desc) = ex.get_string(EXTH_DESCRIPTION) {
            book.metadata.description = Some(desc);
        }

        // ISBN.
        if let Some(isbn) = ex.get_string(EXTH_ISBN) {
            book.metadata.isbn = Some(isbn);
        }

        // Subjects.
        let subjects = ex.get_all_strings(EXTH_SUBJECT);
        for subject in subjects {
            if !subject.is_empty() {
                book.metadata.subjects.push(subject);
            }
        }

        // Language.
        if let Some(lang) = ex.get_string(EXTH_LANGUAGE)
            && !lang.is_empty()
        {
            book.metadata.language = Some(lang);
        }
    }
}

/// Extracts image records from the PDB and adds them to the Book.
fn extract_images(pdb: &PdbFile, book: &mut Book, mobi: Option<&MobiHeader>) {
    let first_image = mobi
        .map(|h| h.first_image_index)
        .filter(|&idx| idx != NULL_INDEX)
        .unwrap_or(u32::MAX) as usize;

    if first_image >= pdb.record_count() {
        return;
    }

    let mut image_index = 0u32;

    for i in first_image..pdb.record_count() {
        let Ok(data) = pdb.record_data(i) else {
            continue;
        };

        // Skip non-image sentinel records.
        if is_non_image_record(data) {
            continue;
        }

        // Detect image type from magic bytes.
        let (ext, media_type) = detect_image_type(data);

        let id = format!("image_{}", image_index);
        let href = format!("images/{}.{}", image_index, ext);
        book.add_resource(&id, &href, data.to_vec(), media_type);

        image_index += 1;
    }
}

/// Checks if a record is a known non-image sentinel.
fn is_non_image_record(data: &[u8]) -> bool {
    for sig in NON_IMAGE_SIGS {
        if data.len() >= sig.len() && &data[..sig.len()] == *sig {
            return true;
        }
    }
    false
}

/// Detects image format from magic bytes.
fn detect_image_type(data: &[u8]) -> (&'static str, &'static str) {
    if data.len() >= 3 && &data[0..3] == b"\xFF\xD8\xFF" {
        ("jpg", "image/jpeg")
    } else if data.len() >= 8 && &data[0..8] == b"\x89PNG\r\n\x1a\n" {
        ("png", "image/png")
    } else if data.len() >= 4 && &data[0..4] == b"GIF8" {
        ("gif", "image/gif")
    } else if data.len() >= 4 && &data[0..4] == b"BM\x00\x00" {
        ("bmp", "image/bmp")
    } else if data.len() >= 4 && &data[0..4] == b"RIFF" {
        ("webp", "image/webp")
    } else {
        ("bin", "application/octet-stream")
    }
}

/// Splits MOBI HTML content into chapters.
///
/// MOBI files use `<mbp:pagebreak />` or `<a filepos=...>` for chapter breaks.
/// This is a simplified splitter that looks for common patterns.
fn split_mobi_content(html: &str) -> Vec<SimpleChapter> {
    let mut chapters = Vec::new();

    // Split on <mbp:pagebreak /> or <mbp:pagebreak/>.
    let parts: Vec<&str> = split_on_pagebreaks(html);

    if parts.len() <= 1 {
        // No page breaks — single chapter.
        chapters.push(SimpleChapter {
            title: None,
            content: html.to_string(),
        });
        return chapters;
    }

    for (i, part) in parts.iter().enumerate() {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Try to extract a title from the first heading.
        let title = extract_first_heading(trimmed);

        chapters.push(SimpleChapter {
            title: title.or_else(|| Some(format!("Chapter {}", i + 1))),
            content: trimmed.to_string(),
        });
    }

    if chapters.is_empty() {
        chapters.push(SimpleChapter {
            title: None,
            content: html.to_string(),
        });
    }

    chapters
}

/// A simple chapter extracted from MOBI content.
struct SimpleChapter {
    title: Option<String>,
    content: String,
}

/// Splits HTML on `<mbp:pagebreak` tags.
fn split_on_pagebreaks(html: &str) -> Vec<&str> {
    let lower = html.to_lowercase();
    let mut parts = Vec::new();
    let mut last = 0;

    let needle = "<mbp:pagebreak";
    for (idx, _) in lower.match_indices(needle) {
        if idx > last {
            parts.push(&html[last..idx]);
        }
        // Find the end of this tag.
        if let Some(end) = html[idx..].find('>') {
            last = idx + end + 1;
        } else {
            last = idx + needle.len();
        }
    }

    if last < html.len() {
        parts.push(&html[last..]);
    }

    if parts.is_empty() {
        parts.push(html);
    }

    parts
}

/// Extracts the text content of the first `<h1>...<h3>` tag.
fn extract_first_heading(html: &str) -> Option<String> {
    let lower = html.to_lowercase();

    for tag in &["<h1", "<h2", "<h3"] {
        if let Some(start_idx) = lower.find(tag) {
            // Find end of opening tag.
            let content_start = html[start_idx..].find('>')? + start_idx + 1;
            // Find closing tag.
            let close_tag = format!("</h{}", &tag[2..]);
            let content_end = lower[content_start..].find(&close_tag)? + content_start;

            let heading_html = &html[content_start..content_end];
            let text = strip_html_tags(heading_html).trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }

    None
}

/// Very simple HTML tag stripper for heading extraction.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;

    for c in html.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(c);
        }
    }

    result
}

/// Decodes CP-1252 bytes to a UTF-8 string.
fn decode_cp1252(data: &[u8]) -> String {
    data.iter()
        .map(|&b| {
            match b {
                0x80 => '\u{20AC}', // Euro sign
                0x82 => '\u{201A}', // Single low-9 quotation
                0x83 => '\u{0192}', // Latin small f with hook
                0x84 => '\u{201E}', // Double low-9 quotation
                0x85 => '\u{2026}', // Horizontal ellipsis
                0x86 => '\u{2020}', // Dagger
                0x87 => '\u{2021}', // Double dagger
                0x88 => '\u{02C6}', // Modifier circumflex
                0x89 => '\u{2030}', // Per mille
                0x8A => '\u{0160}', // S with caron
                0x8B => '\u{2039}', // Single left-pointing angle
                0x8C => '\u{0152}', // OE ligature
                0x8E => '\u{017D}', // Z with caron
                0x91 => '\u{2018}', // Left single quotation
                0x92 => '\u{2019}', // Right single quotation
                0x93 => '\u{201C}', // Left double quotation
                0x94 => '\u{201D}', // Right double quotation
                0x95 => '\u{2022}', // Bullet
                0x96 => '\u{2013}', // En dash
                0x97 => '\u{2014}', // Em dash
                0x98 => '\u{02DC}', // Small tilde
                0x99 => '\u{2122}', // Trade mark
                0x9A => '\u{0161}', // s with caron
                0x9B => '\u{203A}', // Single right-pointing angle
                0x9C => '\u{0153}', // oe ligature
                0x9E => '\u{017E}', // z with caron
                0x9F => '\u{0178}', // Y with diaeresis
                _ => b as char,     // ASCII and Latin-1 supplement
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::common::compression::palmdoc;
    use crate::formats::common::palm_db::{build_pdb_header, write_u16_be, write_u32_be};

    /// Builds a complete minimal MOBI file in memory for testing.
    fn build_test_mobi(title: &str, text: &str, authors: &[&str]) -> Vec<u8> {
        let text_bytes = text.as_bytes();

        // Compress text into records of up to 4096 bytes.
        let mut text_records: Vec<Vec<u8>> = Vec::new();
        let mut offset = 0;
        while offset < text_bytes.len() {
            let end = (offset + palmdoc::RECORD_SIZE).min(text_bytes.len());
            let chunk = &text_bytes[offset..end];
            text_records.push(palmdoc::compress(chunk));
            offset = end;
        }

        if text_records.is_empty() {
            text_records.push(Vec::new());
        }

        // Build EXTH header.
        let mut exth_items: Vec<(u32, Vec<u8>)> = Vec::new();
        for author in authors {
            exth_items.push((exth::EXTH_AUTHOR, author.as_bytes().to_vec()));
        }
        let exth_refs: Vec<(u32, &[u8])> =
            exth_items.iter().map(|(t, d)| (*t, d.as_slice())).collect();
        let exth_data = exth::build_exth(&exth_refs);

        // Build Record 0.
        let mobi_header_len: u32 = 228;
        let title_bytes = title.as_bytes();
        let title_offset = 16 + mobi_header_len + exth_data.len() as u32;
        let record0_len = title_offset as usize + title_bytes.len();
        // Pad to 4-byte alignment.
        let record0_padded = (record0_len + 3) & !3;

        let mut record0 = vec![0u8; record0_padded];

        // PalmDOC header.
        write_u16_be(&mut record0, 0, COMPRESSION_PALMDOC);
        write_u32_be(&mut record0, 4, text_bytes.len() as u32);
        write_u16_be(&mut record0, 8, text_records.len() as u16);
        write_u16_be(&mut record0, 10, 4096);

        // MOBI header.
        record0[16..20].copy_from_slice(b"MOBI");
        write_u32_be(&mut record0, 20, mobi_header_len);
        write_u32_be(&mut record0, 24, 2); // type = book
        write_u32_be(&mut record0, 28, 65001); // UTF-8
        write_u32_be(&mut record0, 36, 6); // version 6

        // First non-book record.
        let first_non_book = 1 + text_records.len() as u32;
        write_u32_be(&mut record0, 80, first_non_book);

        // Full name.
        write_u32_be(&mut record0, 84, title_offset);
        write_u32_be(&mut record0, 88, title_bytes.len() as u32);

        // First image index = NULL (no images in test).
        write_u32_be(&mut record0, 108, NULL_INDEX);

        // EXTH flags (bit 6 set if we have EXTH).
        let exth_flags: u32 = if !exth_data.is_empty() { 0x40 } else { 0 };
        write_u32_be(&mut record0, 128, exth_flags);

        // DRM offset = NULL.
        write_u32_be(&mut record0, 168, NULL_INDEX);

        // Write EXTH after MOBI header.
        let exth_offset = 16 + mobi_header_len as usize;
        if !exth_data.is_empty() {
            record0[exth_offset..exth_offset + exth_data.len()].copy_from_slice(&exth_data);
        }

        // Write title.
        record0[title_offset as usize..title_offset as usize + title_bytes.len()]
            .copy_from_slice(title_bytes);

        // Collect all records.
        let num_records = 1 + text_records.len();
        let header_table_size = 78 + num_records * 8 + 2;

        // Calculate offsets.
        let mut offsets = Vec::with_capacity(num_records);
        let mut pos = header_table_size as u32;
        offsets.push(pos);
        pos += record0.len() as u32;
        for tr in &text_records {
            offsets.push(pos);
            pos += tr.len() as u32;
        }

        // Build PDB header.
        let mut file_data = build_pdb_header(title, b"BOOK", b"MOBI", num_records as u16, &offsets);

        // Append records.
        file_data.extend_from_slice(&record0);
        for tr in &text_records {
            file_data.extend_from_slice(tr);
        }

        file_data
    }

    #[test]
    fn mobi_reader_parses_title_and_content() {
        let mobi_data = build_test_mobi("Test Book", "<html><body><p>Hello MOBI</p></body></html>", &["Test Author"]);

        let mut cursor = std::io::Cursor::new(mobi_data);
        let book = MobiReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Test Book"));
        assert!(!book.metadata.authors.is_empty());
        assert_eq!(book.metadata.authors[0], "Test Author");

        let chapters = book.chapters();
        assert!(!chapters.is_empty());

        let all_content: String = chapters.iter().map(|c| c.content.clone()).collect();
        assert!(all_content.contains("Hello MOBI"));
    }

    #[test]
    fn mobi_reader_handles_multiple_authors() {
        let mobi_data = build_test_mobi("Multi Author", "<p>Content</p>", &["Alice", "Bob"]);

        let mut cursor = std::io::Cursor::new(mobi_data);
        let book = MobiReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.authors.len(), 2);
        assert_eq!(book.metadata.authors[0], "Alice");
        assert_eq!(book.metadata.authors[1], "Bob");
    }

    #[test]
    fn mobi_reader_rejects_non_mobi() {
        let bad_data = vec![0u8; 200];
        let mut cursor = std::io::Cursor::new(bad_data);
        let result = MobiReader::new().read_book(&mut cursor);
        assert!(result.is_err());
    }

    #[test]
    fn mobi_reader_handles_long_text() {
        // Text longer than one record (4096 bytes).
        let long_text = "<p>".to_string() + &"A".repeat(8000) + "</p>";
        let mobi_data = build_test_mobi("Long Book", &long_text, &["Author"]);

        let mut cursor = std::io::Cursor::new(mobi_data);
        let book = MobiReader::new().read_book(&mut cursor).unwrap();

        let chapters = book.chapters();
        let all_content: String = chapters.iter().map(|c| c.content.clone()).collect();
        assert!(all_content.contains(&"A".repeat(100)));
    }

    #[test]
    fn split_on_pagebreaks_works() {
        let html = "part1<mbp:pagebreak />part2<mbp:pagebreak/>part3";
        let parts = split_on_pagebreaks(html);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "part1");
        assert_eq!(parts[1], "part2");
        assert_eq!(parts[2], "part3");
    }

    #[test]
    fn split_on_pagebreaks_no_breaks() {
        let html = "just content";
        let parts = split_on_pagebreaks(html);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], "just content");
    }

    #[test]
    fn extract_heading_from_html() {
        let html = "<h1>Chapter One</h1><p>Content here</p>";
        assert_eq!(extract_first_heading(html), Some("Chapter One".into()));
    }

    #[test]
    fn extract_heading_with_inner_tags() {
        let html = "<h2><b>Bold Title</b></h2>";
        assert_eq!(extract_first_heading(html), Some("Bold Title".into()));
    }

    #[test]
    fn decode_cp1252_basic() {
        let input = &[0x93, 0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x94]; // "Hello"
        let result = decode_cp1252(input);
        assert_eq!(result, "\u{201C}Hello\u{201D}");
    }

    #[test]
    fn detect_jpeg() {
        let data = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00];
        assert_eq!(detect_image_type(data), ("jpg", "image/jpeg"));
    }

    #[test]
    fn detect_png() {
        let data = b"\x89PNG\r\n\x1a\nmore";
        assert_eq!(detect_image_type(data), ("png", "image/png"));
    }
}
