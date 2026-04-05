//! MOBI file writer.
//!
//! Produces a valid MOBI 6 file with PalmDoc compression, EXTH metadata,
//! and embedded images. The output is compatible with Kindle readers.

use crate::domain::Book;
use crate::error::Result;
use crate::formats::common::compression::palmdoc;
use crate::formats::common::html_utils::strip_tags;
use crate::formats::common::palm_db::{build_pdb_header, write_u16_be, write_u32_be};

use image::imageops::FilterType;
use image::ImageEncoder;

use super::exth::{
    self, EXTH_ASIN, EXTH_AUTHOR, EXTH_CDE_TYPE, EXTH_COVER_OFFSET, EXTH_DESCRIPTION, EXTH_ISBN,
    EXTH_LANGUAGE, EXTH_PUBLISHED_DATE, EXTH_PUBLISHER, EXTH_RIGHTS, EXTH_SUBJECT,
    EXTH_THUMB_OFFSET, EXTH_UPDATED_TITLE,
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

/// Maximum thumbnail width for Kindle library display.
const THUMBNAIL_MAX_WIDTH: u32 = 180;
/// Maximum thumbnail height for Kindle library display.
const THUMBNAIL_MAX_HEIGHT: u32 = 240;
/// JPEG quality for generated thumbnails (0-100).
const THUMBNAIL_JPEG_QUALITY: u8 = 75;

/// Generates a downscaled JPEG thumbnail from cover image data.
///
/// The thumbnail fits within [`THUMBNAIL_MAX_WIDTH`] x [`THUMBNAIL_MAX_HEIGHT`]
/// while maintaining the original aspect ratio. Returns `None` if the image
/// cannot be decoded (graceful fallback to using the cover as thumbnail).
fn generate_thumbnail(image_data: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(image_data).ok()?;

    let (orig_w, orig_h) = (img.width(), img.height());
    if orig_w == 0 || orig_h == 0 {
        return None;
    }

    // If the image is already within thumbnail bounds, skip generation entirely
    // to avoid lossy re-encoding. The caller falls back to using the cover itself.
    if orig_w <= THUMBNAIL_MAX_WIDTH && orig_h <= THUMBNAIL_MAX_HEIGHT {
        return None;
    }

    let resized = img.resize(
        THUMBNAIL_MAX_WIDTH,
        THUMBNAIL_MAX_HEIGHT,
        FilterType::Triangle,
    );

    let rgb = resized.to_rgb8();
    let mut jpeg_buf = std::io::Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
        &mut jpeg_buf,
        THUMBNAIL_JPEG_QUALITY,
    );
    encoder
        .write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(jpeg_buf.into_inner())
}

/// Generates a complete MOBI file from a `Book` and returns the raw bytes.
pub(crate) fn write_mobi(book: &Book) -> Result<Vec<u8>> {
    // Convert book content to HTML.
    let html = book_to_mobi_html(book);
    let text_bytes = html.as_bytes();

    // Split and compress text records.
    let text_records = compress_text_records(text_bytes);
    let text_record_count = text_records.len();

    // Collect image data references (borrow from book, avoid cloning).
    let image_refs = collect_image_refs(book);
    let has_images = !image_refs.is_empty();

    // Generate a downscaled thumbnail from the cover (first) image.
    // If generation succeeds the thumbnail is appended as the last image
    // record (after all original images) so that existing recindex references
    // in the HTML are not shifted.  EXTH 202 points to that final index.
    let thumbnail_data = if has_images {
        generate_thumbnail(image_refs[0])
    } else {
        None
    };
    let thumb_offset: u32 = if thumbnail_data.is_some() {
        image_refs.len() as u32 // index after the last original image
    } else {
        0
    };

    // Build EXTH.
    let exth_data = build_metadata_exth(book, has_images, thumb_offset);

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

    // Calculate total number of records and pre-compute total output size.
    let thumbnail_record_count = if thumbnail_data.is_some() { 1 } else { 0 };
    let num_records = 1 + text_record_count + image_refs.len() + thumbnail_record_count + 3; // record0 + text + images + thumbnail + FLIS + FCIS + EOF
    let header_table_size = 78 + num_records * 8 + 2;

    // Calculate record offsets and total data size in a single pass.
    let mut offsets = Vec::with_capacity(num_records);
    let mut pos = header_table_size as u32;

    // Record 0
    offsets.push(pos);
    pos += record0.len() as u32;

    // Text records
    for tr in &text_records {
        offsets.push(pos);
        pos += tr.len() as u32;
    }

    // Image records: all original images first, then thumbnail (if any).
    if !image_refs.is_empty() {
        // All original images in order.
        for ir in &image_refs {
            offsets.push(pos);
            pos += ir.len() as u32;
        }

        // Thumbnail appended after all original images.
        if let Some(ref thumb) = thumbnail_data {
            offsets.push(pos);
            pos += thumb.len() as u32;
        }
    }

    // FLIS
    offsets.push(pos);
    pos += FLIS_RECORD.len() as u32;

    // FCIS
    offsets.push(pos);
    pos += fcis.len() as u32;

    // EOF
    offsets.push(pos);
    pos += EOF_RECORD.len() as u32;

    let total_size = pos as usize;

    // Build PDB header.
    let pdb_name = truncate_pdb_name(title);
    let mut output = build_pdb_header(&pdb_name, b"BOOK", b"MOBI", num_records as u16, &offsets);
    output.reserve(total_size - output.len());

    // Append all records in order.
    output.extend_from_slice(&record0);
    for tr in &text_records {
        output.extend_from_slice(tr);
    }
    // Image records: all original images, then thumbnail.
    if !image_refs.is_empty() {
        for ir in &image_refs {
            output.extend_from_slice(ir);
        }
        if let Some(ref thumb) = thumbnail_data {
            output.extend_from_slice(thumb);
        }
    }
    output.extend_from_slice(FLIS_RECORD);
    output.extend_from_slice(&fcis);
    output.extend_from_slice(EOF_RECORD);

    Ok(output)
}

/// Compresses text into PalmDoc-compressed records.
///
/// Uses a reusable `PalmDocCompressor` to amortise the 16 KB hash-chain
/// initialisation cost across all records (instead of re-creating it per record).
/// Also reuses a single output buffer to reduce allocations.
fn compress_text_records(text: &[u8]) -> Vec<Vec<u8>> {
    let num_records = (text.len() + RECORD_SIZE - 1) / RECORD_SIZE.max(1);
    let mut records = Vec::with_capacity(num_records.max(1));
    let mut compressor = palmdoc::PalmDocCompressor::new();
    let mut buf = Vec::with_capacity(RECORD_SIZE);
    let mut offset = 0;

    while offset < text.len() {
        let end = (offset + RECORD_SIZE).min(text.len());
        let chunk = &text[offset..end];
        buf.clear();
        compressor.compress_record_into(chunk, &mut buf);
        records.push(buf.clone());
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

    // Huffman offsets (not used -- PalmDoc compression).
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

/// Builds EXTH header from Book metadata, writing directly into a single buffer.
///
/// `thumb_offset` controls EXTH 202: 0 = same as cover, N = index of separate
/// thumbnail record relative to the first image record.
fn build_metadata_exth(book: &Book, has_cover: bool, thumb_offset: u32) -> Vec<u8> {
    // Collect (type, data_slice) pairs without cloning the data.
    // We need to be careful about the cover offset bytes lifetime.
    let cover_offset_bytes = 0u32.to_be_bytes();
    let thumb_offset_bytes = thumb_offset.to_be_bytes();

    let mut refs: Vec<(u32, &[u8])> = Vec::with_capacity(12);

    // Title.
    if let Some(ref title) = book.metadata.title {
        refs.push((EXTH_UPDATED_TITLE, title.as_bytes()));
    }

    // Authors.
    for author in &book.metadata.authors {
        refs.push((EXTH_AUTHOR, author.as_bytes()));
    }

    // Publisher.
    if let Some(ref publisher) = book.metadata.publisher {
        refs.push((EXTH_PUBLISHER, publisher.as_bytes()));
    }

    // Description.
    if let Some(ref desc) = book.metadata.description {
        refs.push((EXTH_DESCRIPTION, desc.as_bytes()));
    }

    // ISBN.
    if let Some(ref isbn) = book.metadata.isbn {
        refs.push((EXTH_ISBN, isbn.as_bytes()));
    }

    // Subjects.
    for subject in &book.metadata.subjects {
        refs.push((EXTH_SUBJECT, subject.as_bytes()));
    }

    // Language.
    if let Some(ref lang) = book.metadata.language {
        refs.push((EXTH_LANGUAGE, lang.as_bytes()));
    }

    // Publication date (ISO 8601 / RFC 3339).
    let date_string = book
        .metadata
        .publication_date
        .map(|d| d.to_rfc3339());
    if let Some(ref ds) = date_string {
        refs.push((EXTH_PUBLISHED_DATE, ds.as_bytes()));
    }

    // Rights.
    if let Some(ref rights) = book.metadata.rights {
        refs.push((EXTH_RIGHTS, rights.as_bytes()));
    }

    // ASIN / identifier.
    if let Some(ref identifier) = book.metadata.identifier {
        refs.push((EXTH_ASIN, identifier.as_bytes()));
    }

    // Cover offset (first image = index 0).
    if has_cover {
        refs.push((EXTH_COVER_OFFSET, &cover_offset_bytes));
    }

    // Thumbnail offset (separate thumbnail if available, otherwise same as cover).
    if has_cover {
        refs.push((EXTH_THUMB_OFFSET, &thumb_offset_bytes));
    }

    // CDE type = EBOK (ebook).
    refs.push((EXTH_CDE_TYPE, b"EBOK"));

    exth::build_exth(&refs)
}

/// Collects references to image data from Book resources without cloning.
fn collect_image_refs(book: &Book) -> Vec<&[u8]> {
    let resources = book.resources();
    let mut refs = Vec::with_capacity(resources.len());
    for resource in &resources {
        if resource.media_type.starts_with("image/") {
            refs.push(resource.data);
        }
    }
    refs
}

/// Converts Book content to MOBI-compatible HTML.
///
/// Iterates the book's spine/manifest directly to avoid cloning chapter content
/// strings through the `chapters()` API.
fn book_to_mobi_html(book: &Book) -> String {
    // Estimate total size from manifest data (avoiding chapters() clone).
    let estimated: usize = book
        .spine
        .iter()
        .filter_map(|si| {
            let item = book.manifest.get(&si.manifest_id)?;
            Some(item.data.as_text()?.len() + 200)
        })
        .sum::<usize>()
        + 256;
    let mut html = String::with_capacity(estimated.max(4096));
    html.push_str("<html><head><title>");

    let title = book.metadata.title.as_deref().unwrap_or("Untitled");
    push_html_escaped(&mut html, title);
    html.push_str("</title></head><body>\n");

    // Build a quick href -> title lookup from the TOC.
    let toc = &book.toc;

    for (i, spine_item) in book.spine.iter().enumerate() {
        let Some(manifest_item) = book.manifest.get(&spine_item.manifest_id) else {
            continue;
        };
        let Some(content) = manifest_item.data.as_text() else {
            continue;
        };

        if i > 0 {
            html.push_str("<mbp:pagebreak />\n");
        }

        // Look up title from TOC.
        let ch_title = find_toc_title(toc, &manifest_item.href);
        if let Some(ref title) = ch_title {
            html.push_str("<h2>");
            push_html_escaped(&mut html, title);
            html.push_str("</h2>\n");
        }

        // If content already has HTML tags, strip XHTML wrapper and use; otherwise wrap in <p>.
        if content.contains('<') {
            let cleaned = strip_xhtml_wrapper(content);
            html.push_str(&cleaned);
        } else {
            let plain = strip_tags(content);
            for line in plain.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    html.push_str("<p>");
                    push_html_escaped(&mut html, trimmed);
                    html.push_str("</p>\n");
                }
            }
        }

        html.push('\n');
    }

    html.push_str("</body></html>");
    html
}

/// Searches the TOC for an entry whose href matches (prefix match).
fn find_toc_title(items: &[crate::domain::toc::TocItem], href: &str) -> Option<String> {
    for item in items {
        if item.href == href || href.starts_with(&item.href) {
            return Some(item.title.clone());
        }
        if let Some(title) = find_toc_title(&item.children, href) {
            return Some(title);
        }
    }
    None
}

/// Pushes HTML-escaped text directly into an existing String buffer,
/// avoiding allocation when no escaping is needed (the common case).
#[inline]
fn push_html_escaped(buf: &mut String, text: &str) {
    let escaped = crate::formats::common::text_utils::escape_html(text);
    buf.push_str(&escaped);
}

/// Strips XHTML wrapper elements from chapter content so that only the inner
/// body content remains. This removes XML processing instructions, DOCTYPE
/// declarations, `<html>`, `<head>` (with contents), and `<body>` tags that
/// are present in EPUB XHTML source files.
fn strip_xhtml_wrapper(content: &str) -> String {
    let mut s = content.to_string();

    // Remove <?xml ...?> processing instructions.
    while let Some(start) = s.find("<?xml") {
        if let Some(end) = s[start..].find("?>") {
            s.replace_range(start..start + end + 2, "");
        } else {
            break;
        }
    }

    // Remove <!DOCTYPE ...> declarations.
    while let Some(start) = s.find("<!DOCTYPE") {
        if let Some(end) = s[start..].find('>') {
            s.replace_range(start..start + end + 1, "");
        } else {
            break;
        }
    }
    // Also handle lowercase variant.
    while let Some(start) = s.find("<!doctype") {
        if let Some(end) = s[start..].find('>') {
            s.replace_range(start..start + end + 1, "");
        } else {
            break;
        }
    }

    // Remove <head>...</head> blocks (including contents).
    while let Some(start) = s.find("<head") {
        if let Some(end) = s[start..].find("</head>") {
            s.replace_range(start..start + end + 7, "");
        } else if let Some(end) = s[start..].find("/>") {
            // Self-closing <head/>
            s.replace_range(start..start + end + 2, "");
        } else {
            break;
        }
    }

    // Remove <html ...> and </html> tags.
    while let Some(start) = s.find("<html") {
        if let Some(end) = s[start..].find('>') {
            s.replace_range(start..start + end + 1, "");
        } else {
            break;
        }
    }
    while let Some(start) = s.find("</html>") {
        s.replace_range(start..start + 7, "");
    }

    // Remove <body ...> and </body> tags.
    while let Some(start) = s.find("<body") {
        if let Some(end) = s[start..].find('>') {
            s.replace_range(start..start + end + 1, "");
        } else {
            break;
        }
    }
    while let Some(start) = s.find("</body>") {
        s.replace_range(start..start + 7, "");
    }

    s
}

/// Truncates a title to fit the 31-character PDB name field.
/// Spaces are replaced with underscores per PalmOS convention (matches Calibre).
fn truncate_pdb_name(title: &str) -> String {
    // Fast path: check if title is already valid (all ASCII graphic or space, len <= 31).
    if title.len() <= 31
        && !title.is_empty()
        && title
            .bytes()
            .all(|b| b.is_ascii_graphic() || b == b' ')
    {
        return title.replace(' ', "_");
    }

    let clean: String = title
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(31)
        .collect();

    if clean.is_empty() {
        "Untitled".to_string()
    } else {
        clean.replace(' ', "_")
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
        let mut buf = String::new();
        push_html_escaped(&mut buf, "a & b < c > d");
        assert_eq!(buf, "a &amp; b &lt; c &gt; d");
    }

    #[test]
    fn mobi_round_trip_extended_metadata() {
        use chrono::NaiveDate;

        let mut book = Book::new();
        book.metadata.title = Some("Extended Meta".into());
        book.metadata.authors.push("Bob".into());
        book.metadata.publication_date = Some(
            NaiveDate::from_ymd_opt(2024, 6, 15)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
        );
        book.metadata.rights = Some("CC BY 4.0".into());
        book.metadata.identifier = Some("B00TEST1234".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Content for extended metadata test.</p>".into(),
            id: Some("ch1".into()),
        });

        // Write.
        let mobi_data = write_mobi(&book).unwrap();

        // Read back.
        let mut cursor = std::io::Cursor::new(mobi_data);
        let decoded = MobiReader::new().read_book(&mut cursor).unwrap();

        // Verify the three new fields round-trip correctly.
        assert!(
            decoded.metadata.publication_date.is_some(),
            "publication_date should be present after round-trip"
        );
        let decoded_date = decoded.metadata.publication_date.unwrap();
        assert_eq!(decoded_date.format("%Y-%m-%d").to_string(), "2024-06-15");

        assert_eq!(
            decoded.metadata.rights.as_deref(),
            Some("CC BY 4.0"),
            "rights should round-trip"
        );
        assert_eq!(
            decoded.metadata.identifier.as_deref(),
            Some("B00TEST1234"),
            "identifier should round-trip"
        );
    }

    #[test]
    fn cover_image_writes_exth_201_and_202() {
        use crate::formats::mobi::exth::{ExthHeader, EXTH_COVER_OFFSET, EXTH_THUMB_OFFSET};
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("Cover Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        // Create a real decodable image (300x400 JPEG) so thumbnail generation succeeds.
        let cover_jpeg = create_test_jpeg(300, 400);
        book.add_resource("cover", "cover.jpg", cover_jpeg, "image/jpeg");

        let mobi_data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(mobi_data).unwrap();
        let record0 = pdb.record_data(0).unwrap();

        // Parse MOBI header to locate EXTH.
        let mobi_hdr = MobiHeader::parse(record0).unwrap();
        assert!(mobi_hdr.has_exth(), "EXTH flag should be set");

        let exth_start = mobi_hdr.exth_offset();
        let exth = ExthHeader::parse(&record0[exth_start..]).unwrap();

        // EXTH 201 (cover offset) should be present and equal to 0.
        let cover_offset = exth
            .get_u32(EXTH_COVER_OFFSET)
            .expect("EXTH 201 (cover offset) should be present");
        assert_eq!(cover_offset, 0, "cover offset should be 0");

        // EXTH 202 (thumbnail offset) should be 1 (separate thumbnail record).
        let thumb_offset = exth
            .get_u32(EXTH_THUMB_OFFSET)
            .expect("EXTH 202 (thumbnail offset) should be present");
        assert_eq!(thumb_offset, 1, "thumbnail offset should be 1 (separate thumbnail)");
    }

    #[test]
    fn cover_image_undecodable_falls_back_to_thumb_offset_zero() {
        use crate::formats::mobi::exth::{ExthHeader, EXTH_COVER_OFFSET, EXTH_THUMB_OFFSET};
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("Fallback Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        // Add an undecodable fake JPEG — thumbnail generation should fail gracefully.
        let fake_jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x02, 0xFF, 0xD9];
        book.add_resource("cover", "cover.jpg", fake_jpeg, "image/jpeg");

        let mobi_data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(mobi_data).unwrap();
        let record0 = pdb.record_data(0).unwrap();

        let mobi_hdr = MobiHeader::parse(record0).unwrap();
        let exth_start = mobi_hdr.exth_offset();
        let exth = ExthHeader::parse(&record0[exth_start..]).unwrap();

        // Cover offset is still 0.
        let cover_offset = exth
            .get_u32(EXTH_COVER_OFFSET)
            .expect("EXTH 201 should be present");
        assert_eq!(cover_offset, 0);

        // Thumbnail offset falls back to 0 (same as cover).
        let thumb_offset = exth
            .get_u32(EXTH_THUMB_OFFSET)
            .expect("EXTH 202 should be present");
        assert_eq!(thumb_offset, 0, "thumbnail offset should fall back to 0");
    }

    #[test]
    fn thumbnail_record_exists_and_is_smaller_than_cover() {
        let mut book = Book::new();
        book.metadata.title = Some("Thumb Size Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        // Create a 600x800 JPEG cover (large enough that the thumbnail will be smaller).
        let cover_jpeg = create_test_jpeg(600, 800);
        let cover_size = cover_jpeg.len();
        book.add_resource("cover", "cover.jpg", cover_jpeg, "image/jpeg");

        let mobi_data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(mobi_data).unwrap();

        // Record layout: [record0, text..., cover, thumbnail, FLIS, FCIS, EOF]
        // With 1 text record: record0=0, text=1, cover=2, thumbnail=3, FLIS=4, FCIS=5, EOF=6
        let num_records = pdb.record_count();
        assert!(num_records >= 7, "should have at least 7 records (record0 + text + cover + thumb + FLIS + FCIS + EOF), got {num_records}");

        // The cover is at index 2 (after record0 and 1 text record).
        let cover_record = pdb.record_data(2).unwrap();
        assert_eq!(cover_record.len(), cover_size, "cover record should match original cover data");

        // The thumbnail is at index 3.
        let thumb_record = pdb.record_data(3).unwrap();
        assert!(
            thumb_record.len() < cover_record.len(),
            "thumbnail ({} bytes) should be smaller than cover ({} bytes)",
            thumb_record.len(),
            cover_record.len()
        );

        // Verify the thumbnail is a valid JPEG (starts with FFD8).
        assert_eq!(&thumb_record[..2], &[0xFF, 0xD8], "thumbnail should be a valid JPEG");

        // Verify thumbnail dimensions are within bounds.
        let thumb_img = image::load_from_memory(thumb_record).expect("thumbnail should be decodable");
        assert!(thumb_img.width() <= 180, "thumbnail width {} should be <= 180", thumb_img.width());
        assert!(thumb_img.height() <= 240, "thumbnail height {} should be <= 240", thumb_img.height());
    }

    /// Creates a test JPEG image of the given dimensions using the image crate.
    fn create_test_jpeg(width: u32, height: u32) -> Vec<u8> {
        use image::ImageEncoder;
        let img = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        let mut buf = std::io::Cursor::new(Vec::new());
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90);
        encoder
            .write_image(
                img.as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        buf.into_inner()
    }

    #[test]
    fn thumbnail_appended_after_all_images_no_index_shift() {
        // Regression test: thumbnail must be appended AFTER all original images
        // so that recindex references in the HTML are not shifted.
        use crate::formats::mobi::exth::{ExthHeader, EXTH_THUMB_OFFSET};
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("Multi-Image Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        // All images are larger than 180x240 so that whichever one ends up
        // first (HashMap ordering) will still trigger thumbnail generation.
        let cover_jpeg = create_test_jpeg(600, 800);
        let img2_jpeg = create_test_jpeg(300, 400);
        let img3_jpeg = create_test_jpeg(400, 300);
        book.add_resource("cover", "cover.jpg", cover_jpeg, "image/jpeg");
        book.add_resource("img2", "img2.jpg", img2_jpeg, "image/jpeg");
        book.add_resource("img3", "img3.jpg", img3_jpeg, "image/jpeg");

        let mobi_data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(mobi_data).unwrap();

        // Record layout: [record0, text, img*3, thumbnail, FLIS, FCIS, EOF] = 9 records
        assert_eq!(pdb.record_count(), 9, "expected 9 records (1+1+3+1+3), got {}", pdb.record_count());

        // The thumbnail is the 4th image record (index 5 = record0 + 1 text + 3 images).
        // It's a JPEG and should be smaller than the large cover.
        let thumb_record = pdb.record_data(5).unwrap();
        assert_eq!(&thumb_record[..2], &[0xFF, 0xD8], "thumbnail should be a valid JPEG");

        // EXTH 202 should point to index 3 (= number of original images, 0-based from first image).
        let record0 = pdb.record_data(0).unwrap();
        let mobi_hdr = MobiHeader::parse(record0).unwrap();
        let exth_start = mobi_hdr.exth_offset();
        let exth = ExthHeader::parse(&record0[exth_start..]).unwrap();
        let thumb_offset = exth.get_u32(EXTH_THUMB_OFFSET).expect("EXTH 202 should be present");
        assert_eq!(thumb_offset, 3, "thumbnail offset should be 3 (after 3 original images)");
    }

    #[test]
    fn small_cover_skips_thumbnail_generation() {
        // If the cover is already within 180x240 bounds, no separate thumbnail
        // should be generated (avoids lossy re-encoding).
        use crate::formats::mobi::exth::{ExthHeader, EXTH_THUMB_OFFSET};
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("Small Cover Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        // 100x100 is within 180x240 bounds — should NOT generate a thumbnail.
        let small_jpeg = create_test_jpeg(100, 100);
        book.add_resource("cover", "cover.jpg", small_jpeg, "image/jpeg");

        let mobi_data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(mobi_data).unwrap();

        // Record layout without thumbnail: [record0, text, cover, FLIS, FCIS, EOF] = 6 records
        assert_eq!(pdb.record_count(), 6, "no thumbnail record should be generated for small cover");

        // EXTH 202 should fall back to 0 (same as cover).
        let record0 = pdb.record_data(0).unwrap();
        let mobi_hdr = MobiHeader::parse(record0).unwrap();
        let exth_start = mobi_hdr.exth_offset();
        let exth = ExthHeader::parse(&record0[exth_start..]).unwrap();
        let thumb_offset = exth.get_u32(EXTH_THUMB_OFFSET).expect("EXTH 202 should be present");
        assert_eq!(thumb_offset, 0, "thumbnail offset should be 0 (no separate thumbnail)");
    }

    #[test]
    fn no_cover_omits_exth_201_and_202() {
        use crate::formats::mobi::exth::{ExthHeader, EXTH_COVER_OFFSET, EXTH_THUMB_OFFSET};
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("No Cover Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });
        // No image resources added.

        let mobi_data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(mobi_data).unwrap();
        let record0 = pdb.record_data(0).unwrap();

        let mobi_hdr = MobiHeader::parse(record0).unwrap();
        assert!(mobi_hdr.has_exth(), "EXTH flag should be set");

        let exth_start = mobi_hdr.exth_offset();
        let exth = ExthHeader::parse(&record0[exth_start..]).unwrap();

        // Neither EXTH 201 nor 202 should be present when there is no cover.
        assert!(
            exth.get_u32(EXTH_COVER_OFFSET).is_none(),
            "EXTH 201 should not be present without a cover image"
        );
        assert!(
            exth.get_u32(EXTH_THUMB_OFFSET).is_none(),
            "EXTH 202 should not be present without a cover image"
        );
    }
}
