//! MOBI file writer.
//!
//! Produces a valid MOBI 6 file with PalmDoc compression, EXTH metadata,
//! and embedded images. The output is compatible with Kindle readers.

use crate::domain::Book;
use crate::error::Result;
use crate::formats::common::compression::palmdoc;
use crate::formats::common::html_utils::strip_tags;
use crate::formats::common::palm_db::{build_pdb_header, write_u16_be, write_u32_be};

use super::exth::{
    self, EXTH_AUTHOR, EXTH_CDE_TYPE, EXTH_COVER_OFFSET, EXTH_DESCRIPTION, EXTH_ISBN,
    EXTH_LANGUAGE, EXTH_PUBLISHER, EXTH_SUBJECT, EXTH_UPDATED_TITLE,
};
use super::header::{COMPRESSION_PALMDOC, ENCODING_UTF8, NULL_INDEX};

/// Maximum uncompressed text record size.
const RECORD_SIZE: usize = 4096;

/// MOBI 6 header length.
const MOBI_HEADER_LEN: u32 = 228;

/// FLIS record constant data.
const FLIS_RECORD: &[u8] = &[
    b'F', b'L', b'I', b'S', 0x00, 0x00, 0x00, 0x08, 0x00, 0x41, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x01, 0x00, 0x03, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01,
    0xFF, 0xFF, 0xFF, 0xFF,
];

/// FCIS record (parameterized by text length).
fn build_fcis(text_length: u32) -> Vec<u8> {
    let mut data = vec![0u8; 44];
    data[0..4].copy_from_slice(b"FCIS");
    write_u32_be(&mut data, 4, 0x14); // FCIS data offset
    write_u32_be(&mut data, 8, 0x10); // unknown
    write_u32_be(&mut data, 12, 0x01);
    write_u32_be(&mut data, 16, 0x00);
    write_u32_be(&mut data, 20, text_length);
    write_u32_be(&mut data, 24, 0x00);
    write_u32_be(&mut data, 28, 0x20);
    write_u32_be(&mut data, 32, 0x08);
    write_u32_be(&mut data, 36, 0x01);
    write_u32_be(&mut data, 40, 0x01);
    data
}

/// EOF record.
const EOF_RECORD: &[u8] = &[0xE9, 0x8E, 0x0D, 0x0A];

/// Generates a complete MOBI file from a `Book` and returns the raw bytes.
pub(crate) fn write_mobi(book: &Book) -> Result<Vec<u8>> {
    // Convert book content to HTML.
    let html = book_to_mobi_html(book);
    let text_bytes = html.as_bytes();

    // Split and compress text records.
    let text_records = compress_text_records(text_bytes);
    let text_record_count = text_records.len();

    // Build image records.
    let image_records = build_image_records(book);
    let has_images = !image_records.is_empty();

    // Build EXTH.
    let exth_data = build_metadata_exth(book, has_images);

    // Build Record 0.
    let title = book.metadata.title.as_deref().unwrap_or("Untitled");
    let record0 = build_record0(
        title,
        text_bytes.len() as u32,
        text_record_count as u16,
        &exth_data,
        text_record_count as u32 + 1, // first image index (1-based after text records)
        has_images,
    );

    // Structural records: FLIS, FCIS, EOF.
    let fcis = build_fcis(text_bytes.len() as u32);

    // Collect all records: record0, text records, image records, FLIS, FCIS, EOF.
    let mut all_records: Vec<&[u8]> = Vec::new();
    all_records.push(&record0);
    for tr in &text_records {
        all_records.push(tr);
    }
    for ir in &image_records {
        all_records.push(ir);
    }
    all_records.push(FLIS_RECORD);
    all_records.push(&fcis);
    all_records.push(EOF_RECORD);

    let num_records = all_records.len() as u16;

    // Calculate record offsets.
    let header_table_size = 78 + (num_records as usize) * 8 + 2;
    let mut offsets = Vec::with_capacity(all_records.len());
    let mut pos = header_table_size as u32;
    for rec in &all_records {
        offsets.push(pos);
        pos += rec.len() as u32;
    }

    // Build PDB header.
    let pdb_name = truncate_pdb_name(title);
    let mut output = build_pdb_header(&pdb_name, b"BOOK", b"MOBI", num_records, &offsets);

    // Append all records.
    for rec in &all_records {
        output.extend_from_slice(rec);
    }

    Ok(output)
}

/// Compresses text into PalmDoc-compressed records.
fn compress_text_records(text: &[u8]) -> Vec<Vec<u8>> {
    let mut records = Vec::new();
    let mut offset = 0;

    while offset < text.len() {
        let end = (offset + RECORD_SIZE).min(text.len());
        let chunk = &text[offset..end];
        records.push(palmdoc::compress(chunk));
        offset = end;
    }

    if records.is_empty() {
        records.push(Vec::new());
    }

    records
}

/// Builds Record 0 with PalmDOC + MOBI + EXTH headers + title.
fn build_record0(
    title: &str,
    text_length: u32,
    text_record_count: u16,
    exth_data: &[u8],
    first_image_index: u32,
    has_images: bool,
) -> Vec<u8> {
    let title_bytes = title.as_bytes();
    let exth_len = exth_data.len();
    let title_offset = 16 + MOBI_HEADER_LEN as usize + exth_len;
    let total_size = title_offset + title_bytes.len();
    // Pad to 4-byte alignment.
    let padded_size = (total_size + 3) & !3;

    let mut data = vec![0u8; padded_size];

    // --- PalmDOC header (0-15) ---
    write_u16_be(&mut data, 0, COMPRESSION_PALMDOC);
    // Offset 2-3: unused (0).
    write_u32_be(&mut data, 4, text_length);
    write_u16_be(&mut data, 8, text_record_count);
    write_u16_be(&mut data, 10, RECORD_SIZE as u16);
    // Offset 12-15: encryption=0, unused=0.

    // --- MOBI header (16+) ---
    data[16..20].copy_from_slice(b"MOBI");
    write_u32_be(&mut data, 20, MOBI_HEADER_LEN);
    write_u32_be(&mut data, 24, 2); // mobi_type = book
    write_u32_be(&mut data, 28, ENCODING_UTF8);
    write_u32_be(&mut data, 32, 0x0000_CAFE); // unique ID
    write_u32_be(&mut data, 36, 6); // file version = MOBI 6

    // Offsets 40-79: various index fields (NULL).
    for offset in (40..80).step_by(4) {
        write_u32_be(&mut data, offset, NULL_INDEX);
    }

    // First non-book record.
    let first_non_book = 1 + text_record_count as u32;
    write_u32_be(&mut data, 80, first_non_book);

    // Full name offset + length.
    write_u32_be(&mut data, 84, title_offset as u32);
    write_u32_be(&mut data, 88, title_bytes.len() as u32);

    // Locale = 0x09 (English).
    write_u32_be(&mut data, 92, 0x09);

    // Min version = 6.
    write_u32_be(&mut data, 104, 6);

    // First image index.
    let img_idx = if has_images {
        first_image_index
    } else {
        NULL_INDEX
    };
    write_u32_be(&mut data, 108, img_idx);

    // Huffman offsets (not used — PalmDoc compression).
    write_u32_be(&mut data, 112, 0);
    write_u32_be(&mut data, 116, 0);

    // EXTH flags.
    let exth_flags: u32 = if !exth_data.is_empty() { 0x40 } else { 0 };
    write_u32_be(&mut data, 128, exth_flags);

    // Offsets 132-167: unknown/zeroes.

    // DRM fields (none).
    write_u32_be(&mut data, 168, NULL_INDEX);
    write_u32_be(&mut data, 172, 0);
    write_u32_be(&mut data, 176, 0);
    write_u32_be(&mut data, 180, 0);

    // Extra data flags = 0 (no trailing data).
    write_u32_be(&mut data, 240, 0);

    // NCX index = NULL.
    write_u32_be(&mut data, 244, NULL_INDEX);

    // --- EXTH header ---
    if !exth_data.is_empty() {
        let exth_start = 16 + MOBI_HEADER_LEN as usize;
        data[exth_start..exth_start + exth_len].copy_from_slice(exth_data);
    }

    // --- Title ---
    data[title_offset..title_offset + title_bytes.len()].copy_from_slice(title_bytes);

    data
}

/// Builds EXTH header from Book metadata.
fn build_metadata_exth(book: &Book, has_cover: bool) -> Vec<u8> {
    let mut items: Vec<(u32, Vec<u8>)> = Vec::new();

    // Title.
    if let Some(ref title) = book.metadata.title {
        items.push((EXTH_UPDATED_TITLE, title.as_bytes().to_vec()));
    }

    // Authors.
    for author in &book.metadata.authors {
        items.push((EXTH_AUTHOR, author.as_bytes().to_vec()));
    }

    // Publisher.
    if let Some(ref publisher) = book.metadata.publisher {
        items.push((EXTH_PUBLISHER, publisher.as_bytes().to_vec()));
    }

    // Description.
    if let Some(ref desc) = book.metadata.description {
        items.push((EXTH_DESCRIPTION, desc.as_bytes().to_vec()));
    }

    // ISBN.
    if let Some(ref isbn) = book.metadata.isbn {
        items.push((EXTH_ISBN, isbn.as_bytes().to_vec()));
    }

    // Subjects.
    for subject in &book.metadata.subjects {
        items.push((EXTH_SUBJECT, subject.as_bytes().to_vec()));
    }

    // Language.
    if let Some(ref lang) = book.metadata.language {
        items.push((EXTH_LANGUAGE, lang.as_bytes().to_vec()));
    }

    // Cover offset (first image = index 0).
    if has_cover {
        items.push((EXTH_COVER_OFFSET, 0u32.to_be_bytes().to_vec()));
    }

    // CDE type = EBOK (ebook).
    items.push((EXTH_CDE_TYPE, b"EBOK".to_vec()));

    let refs: Vec<(u32, &[u8])> = items.iter().map(|(t, d)| (*t, d.as_slice())).collect();
    exth::build_exth(&refs)
}

/// Builds image records from Book resources.
fn build_image_records(book: &Book) -> Vec<Vec<u8>> {
    let mut records = Vec::new();

    for resource in &book.resources() {
        if resource.media_type.starts_with("image/") {
            records.push(resource.data.to_vec());
        }
    }

    records
}

/// Converts Book content to MOBI-compatible HTML.
fn book_to_mobi_html(book: &Book) -> String {
    let mut html = String::with_capacity(4096);
    html.push_str("<html><head><title>");

    let title = book.metadata.title.as_deref().unwrap_or("Untitled");
    html.push_str(&html_escape(title));
    html.push_str("</title></head><body>\n");

    let chapters = book.chapters();
    for (i, chapter) in chapters.iter().enumerate() {
        if i > 0 {
            html.push_str("<mbp:pagebreak />\n");
        }

        if let Some(ref ch_title) = chapter.title {
            html.push_str("<h2>");
            html.push_str(&html_escape(ch_title));
            html.push_str("</h2>\n");
        }

        // If content already has HTML tags, use as-is; otherwise wrap in <p>.
        let content = &chapter.content;
        if content.contains('<') {
            html.push_str(content);
        } else {
            let plain = strip_tags(content);
            for line in plain.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    html.push_str("<p>");
                    html.push_str(&html_escape(trimmed));
                    html.push_str("</p>\n");
                }
            }
        }

        html.push('\n');
    }

    html.push_str("</body></html>");
    html
}

/// Basic HTML entity escaping.
fn html_escape(s: &str) -> String {
    crate::formats::common::text_utils::escape_html(s)
}

/// Truncates a title to fit the 31-character PDB name field.
fn truncate_pdb_name(title: &str) -> String {
    let clean: String = title
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(31)
        .collect();

    if clean.is_empty() {
        "Untitled".to_string()
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;
    use crate::domain::FormatReader;
    use crate::formats::common::palm_db::PdbFile;
    use crate::formats::mobi::MobiReader;

    #[test]
    fn write_mobi_produces_valid_pdb() {
        let mut book = Book::new();
        book.metadata.title = Some("Writer Test".into());
        book.metadata.authors.push("Test Author".into());
        book.add_chapter(&Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello MOBI writer!</p>".into(),
            id: Some("ch1".into()),
        });

        let data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(data).unwrap();

        assert!(pdb.header.is_mobi());
        assert!(pdb.record_count() >= 4); // record0 + text + FLIS + FCIS + EOF
    }

    #[test]
    fn mobi_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("Round Trip MOBI".into());
        book.metadata.authors.push("Alice".into());
        book.metadata.language = Some("en".into());
        book.add_chapter(&Chapter {
            title: Some("Intro".into()),
            content: "<p>First chapter content here.</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(&Chapter {
            title: Some("Chapter Two".into()),
            content: "<p>Second chapter with more text.</p>".into(),
            id: Some("ch2".into()),
        });

        // Write.
        let mobi_data = write_mobi(&book).unwrap();

        // Read back.
        let mut cursor = std::io::Cursor::new(mobi_data);
        let decoded = MobiReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("Round Trip MOBI"));
        assert!(decoded.metadata.authors.iter().any(|a| a == "Alice"));

        let chapters = decoded.chapters();
        assert!(!chapters.is_empty());

        let all_content: String = chapters.iter().map(|c| c.content.clone()).collect();
        assert!(all_content.contains("First chapter content"));
        assert!(all_content.contains("Second chapter"));
    }

    #[test]
    fn mobi_round_trip_long_text() {
        let mut book = Book::new();
        book.metadata.title = Some("Long Text".into());

        let long_content = "<p>".to_string() + &"Word ".repeat(2000) + "</p>";
        book.add_chapter(&Chapter {
            title: Some("Big Chapter".into()),
            content: long_content,
            id: Some("ch1".into()),
        });

        let mobi_data = write_mobi(&book).unwrap();
        let mut cursor = std::io::Cursor::new(mobi_data);
        let decoded = MobiReader::new().read_book(&mut cursor).unwrap();

        let chapters = decoded.chapters();
        let all_content: String = chapters.iter().map(|c| c.content.clone()).collect();
        assert!(all_content.contains("Word"));
    }

    #[test]
    fn truncate_pdb_name_works() {
        assert_eq!(truncate_pdb_name("Short"), "Short");
        assert_eq!(truncate_pdb_name(&"A".repeat(50)).len(), 31);
        assert_eq!(truncate_pdb_name(""), "Untitled");
    }

    #[test]
    fn html_escape_works() {
        assert_eq!(html_escape("a & b < c > d"), "a &amp; b &lt; c &gt; d");
    }
}
