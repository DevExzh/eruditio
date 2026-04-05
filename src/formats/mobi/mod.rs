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
use crate::formats::common::palm_db::{read_u32_be, PdbFile};
use crate::formats::common::text_utils;
use crate::formats::common::MAX_INPUT_SIZE;
use std::io::{Read, Write};

use self::exth::{
    EXTH_ASIN, EXTH_AUTHOR, EXTH_DESCRIPTION, EXTH_ISBN, EXTH_LANGUAGE, EXTH_PUBLISHED_DATE,
    EXTH_PUBLISHER, EXTH_RIGHTS, EXTH_SUBJECT, EXTH_UPDATED_TITLE, ExthHeader,
};
use self::header::{
    COMPRESSION_HUFFCDIC, COMPRESSION_NONE, COMPRESSION_PALMDOC, MobiHeader, NULL_INDEX,
    PalmDocHeader,
};

/// Non-text record signatures that should be skipped when extracting images.
const NON_IMAGE_SIGS: &[&[u8]] = &[
    b"FLIS",
    b"FCIS",
    b"SRCS",
    b"RESC",
    b"BOUN",
    b"FDST",
    b"DATP",
    b"AUDI",
    b"VIDE",
    b"\xe9\x8e\r\n",
    b"BOUNDARY",
];

/// Parsed FDST (Flow Descriptor Table) entry.
#[derive(Debug, Clone)]
struct FdstEntry {
    start: usize,
    end: usize,
}

/// Parses the FDST record to get flow byte ranges within the decompressed text.
fn parse_fdst(pdb: &PdbFile, fdst_record_index: usize) -> Option<Vec<FdstEntry>> {
    let data = pdb.record_data(fdst_record_index).ok()?;
    if data.len() < 12 || &data[..4] != b"FDST" {
        return None;
    }
    let num_flows = read_u32_be(data, 8) as usize;
    let mut entries = Vec::with_capacity(num_flows);
    for i in 0..num_flows {
        let pos = 12 + i * 8;
        if pos + 8 > data.len() {
            break;
        }
        let start = read_u32_be(data, pos) as usize;
        let end = read_u32_be(data, pos + 4) as usize;
        entries.push(FdstEntry { start, end });
    }
    Some(entries)
}

/// Finds the FDST record index by scanning PDB records for the "FDST" magic signature.
fn find_fdst_record(pdb: &PdbFile, first_image: usize) -> Option<usize> {
    // Scan records after the image records for the FDST signature.
    // Start from the first image record and look forward.
    for i in first_image..pdb.record_count() {
        if let Ok(data) = pdb.record_data(i) {
            if data.len() >= 4 && &data[..4] == b"FDST" {
                return Some(i);
            }
        }
    }
    None
}

/// Decodes a Kindle base-32 encoded number.
/// Characters: 0-9 -> 0-9, A-V (case-insensitive) -> 10-31.
fn decode_kindle_base32(s: &str) -> Option<usize> {
    let mut result: usize = 0;
    for ch in s.chars() {
        let digit = match ch {
            '0'..='9' => ch as usize - '0' as usize,
            'A'..='V' => ch as usize - 'A' as usize + 10,
            'a'..='v' => ch as usize - 'a' as usize + 10,
            _ => return None,
        };
        result = result.checked_mul(32)?.checked_add(digit)?;
    }
    Some(result)
}

/// Resolves kindle:embed and kindle:flow references in HTML content.
/// Returns the HTML with references replaced by actual resource paths.
///
/// - `image_paths`: indexed by 0-based image record number
/// - `flow_paths`: indexed by flow number (flow 0 = None since it's the main content)
fn resolve_kindle_references(
    html: &str,
    image_paths: &[String],
    flow_paths: &[Option<String>],
) -> String {
    let mut result = String::with_capacity(html.len());
    let mut remaining = html;

    while let Some(pos) = remaining.find("kindle:") {
        result.push_str(&remaining[..pos]);
        let after_kindle = &remaining[pos + 7..]; // skip "kindle:"

        if let Some(replacement) = try_resolve_embed(after_kindle, image_paths) {
            result.push_str(&replacement.0);
            remaining = &remaining[pos + 7 + replacement.1..];
        } else if let Some(replacement) = try_resolve_flow(after_kindle, flow_paths) {
            result.push_str(&replacement.0);
            remaining = &remaining[pos + 7 + replacement.1..];
        } else {
            // Unresolvable reference; keep the "kindle:" prefix and advance past it.
            result.push_str("kindle:");
            remaining = after_kindle;
        }
    }

    result.push_str(remaining);
    result
}

/// Tries to resolve a kindle:embed:XXXX reference.
/// Returns (replacement_string, bytes_consumed_after_"kindle:") or None.
///
/// kindle:embed indices are 1-based: kindle:embed:0001 refers to the first image.
fn try_resolve_embed(after_kindle: &str, image_paths: &[String]) -> Option<(String, usize)> {
    let rest = after_kindle.strip_prefix("embed:")?;
    let consumed_prefix = 6; // "embed:"

    // Extract the base-32 code (alphanumeric characters).
    let code_end = rest
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(rest.len());
    if code_end == 0 {
        return None;
    }
    let code = &rest[..code_end];
    let raw_index = decode_kindle_base32(code)?;

    // kindle:embed indices are 1-based.
    let index = raw_index.checked_sub(1)?;

    // Skip optional ?mime=... query string.
    let mut total_consumed = consumed_prefix + code_end;
    let after_code = &rest[code_end..];
    if let Some(query_rest) = after_code.strip_prefix('?') {
        // Consume until we hit a quote, >, or whitespace (typical attribute terminators).
        let query_end = query_rest
            .find(|c: char| c == '"' || c == '\'' || c == '>' || c.is_ascii_whitespace())
            .unwrap_or(query_rest.len());
        total_consumed += 1 + query_end; // +1 for the '?'
    }

    // Look up the image path.
    let path = image_paths.get(index)?;
    Some((path.clone(), total_consumed))
}

/// Tries to resolve a kindle:flow:NNNN reference.
/// Returns (replacement_string, bytes_consumed_after_"kindle:") or None.
fn try_resolve_flow(after_kindle: &str, flow_paths: &[Option<String>]) -> Option<(String, usize)> {
    let rest = after_kindle.strip_prefix("flow:")?;
    let consumed_prefix = 5; // "flow:"

    // Extract the decimal index.
    let code_end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if code_end == 0 {
        return None;
    }
    let code = &rest[..code_end];
    let index: usize = code.parse().ok()?;

    // Skip optional ?mime=... query string.
    let mut total_consumed = consumed_prefix + code_end;
    let after_code = &rest[code_end..];
    if let Some(query_rest) = after_code.strip_prefix('?') {
        let query_end = query_rest
            .find(|c: char| c == '"' || c == '\'' || c == '>' || c.is_ascii_whitespace())
            .unwrap_or(query_rest.len());
        total_consumed += 1 + query_end;
    }

    // Look up the flow path.
    let path = flow_paths.get(index)?.as_ref()?;
    Some((path.clone(), total_consumed))
}

/// Detects the content type and file extension of a KF8 flow resource.
fn detect_flow_type(data: &[u8]) -> (&'static str, &'static str) {
    let trimmed = trim_start_whitespace(data);
    if trimmed.starts_with(b"<svg") || trimmed.starts_with(b"<SVG") || trimmed.starts_with(b"<?xml") {
        // Could be SVG or XML; check for SVG indicators.
        if data.windows(4).any(|w| w == b"<svg" || w == b"<SVG") {
            ("svg", "image/svg+xml")
        } else {
            ("svg", "image/svg+xml") // default XML to SVG for KF8 flows
        }
    } else {
        // Assume CSS for everything else.
        ("css", "text/css")
    }
}

/// Trims leading ASCII whitespace bytes.
fn trim_start_whitespace(data: &[u8]) -> &[u8] {
    let start = data
        .iter()
        .position(|&b| !b.is_ascii_whitespace())
        .unwrap_or(data.len());
    &data[start..]
}

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
        (&mut *reader).take(MAX_INPUT_SIZE).read_to_end(&mut buffer)?;

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

        // Extract images and collect their paths for reference resolution.
        let image_paths = extract_images_with_paths(&pdb, &mut book, mobi_header.as_ref());

        // For KF8 files, split flows and resolve kindle: references.
        let is_kf8 = mobi_header.as_ref().is_some_and(|h| h.is_kf8());

        let content = if is_kf8 {
            // Try to find and parse the FDST record for flow boundaries.
            let first_image = mobi_header
                .as_ref()
                .map(|h| h.first_image_index as usize)
                .filter(|&idx| idx != NULL_INDEX as usize)
                .unwrap_or(pdb.record_count());

            let fdst_entries = find_fdst_record(&pdb, first_image)
                .and_then(|idx| parse_fdst(&pdb, idx));

            let (main_html_bytes, flow_paths) = if let Some(ref entries) = fdst_entries {
                // Extract flow 0 as main HTML, flows 1+ as resources.
                let main_bytes = if !entries.is_empty() && entries[0].end <= text.len() {
                    &text[entries[0].start..entries[0].end]
                } else {
                    &text[..]
                };

                let mut fpaths: Vec<Option<String>> = Vec::with_capacity(entries.len());
                fpaths.push(None); // Flow 0 is the main content.

                for (i, entry) in entries.iter().enumerate().skip(1) {
                    if entry.start <= text.len() && entry.end <= text.len() && entry.start < entry.end {
                        let flow_data = &text[entry.start..entry.end];
                        let (ext, media_type) = detect_flow_type(flow_data);
                        let flow_id = format!("flow_{}", i);
                        let flow_href = format!("flows/flow_{}.{}", i, ext);
                        book.add_resource(&flow_id, &flow_href, flow_data.to_vec(), media_type);
                        fpaths.push(Some(flow_href));
                    } else {
                        fpaths.push(None);
                    }
                }

                (main_bytes, fpaths)
            } else {
                // No FDST: use all text as HTML content.
                (text.as_slice(), Vec::new())
            };

            // Decode main HTML bytes to string.
            let html_string = if mobi_header.as_ref().is_some_and(|h| h.is_utf8()) {
                crate::formats::common::text_utils::bytes_to_string(main_html_bytes)
            } else {
                decode_cp1252(main_html_bytes)
            };

            // Resolve kindle: references.
            resolve_kindle_references(&html_string, &image_paths, &flow_paths)
        } else {
            // Non-KF8: original behavior.
            if mobi_header.as_ref().is_some_and(|h| h.is_utf8()) {
                crate::formats::common::text_utils::bytes_to_string(&text)
            } else {
                decode_cp1252(&text)
            }
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
        output.write_all(&data)?;
        Ok(())
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

    // Cap pre-allocation to prevent OOM from crafted text_length headers.
    const MAX_PREALLOC: usize = 64 * 1024 * 1024; // 64 MB
    const MAX_TEXT_OUTPUT: usize = 256 * 1024 * 1024; // 256 MB cumulative limit
    let mut text = Vec::with_capacity((palmdoc.text_length as usize).min(MAX_PREALLOC));
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
            },
            COMPRESSION_PALMDOC => {
                palmdoc::decompress_into(record_data, &mut text)?;
                if text.len() > MAX_TEXT_OUTPUT {
                    return Err(EruditioError::Format(
                        "Decompressed text exceeds maximum allowed size".into(),
                    ));
                }
            },
            COMPRESSION_HUFFCDIC => {
                // HUFF/CDIC: lazily initialize the decompressor on first use.
                if huff_reader.is_none() {
                    huff_reader = Some(build_huffcdic_reader(pdb, mobi_header)?);
                }
                let reader = huff_reader.as_mut().ok_or_else(|| {
                    EruditioError::Compression("HUFF/CDIC reader not initialized".into())
                })?;
                let decompressed = reader.unpack(record_data).map_err(|e| {
                    EruditioError::Compression(format!("HUFF/CDIC decompression failed: {}", e))
                })?;
                text.extend_from_slice(&decompressed);
                if text.len() > MAX_TEXT_OUTPUT {
                    return Err(EruditioError::Format(
                        "Decompressed text exceeds maximum allowed size".into(),
                    ));
                }
            },
            other => {
                return Err(EruditioError::Format(format!(
                    "Unknown MOBI compression type: {}",
                    other
                )));
            },
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

        // Publication date.
        if let Some(date_str) = ex.get_string(EXTH_PUBLISHED_DATE)
            && !date_str.is_empty()
        {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&date_str) {
                book.metadata.publication_date = Some(dt.with_timezone(&chrono::Utc));
            } else if let Ok(date) = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
                book.metadata.publication_date = date
                    .and_hms_opt(0, 0, 0)
                    .and_then(|ndt| ndt.and_local_timezone(chrono::Utc).single());
            }
        }

        // Rights.
        if let Some(rights) = ex.get_string(EXTH_RIGHTS)
            && !rights.is_empty()
        {
            book.metadata.rights = Some(rights);
        }

        // Identifier (ASIN).
        if let Some(identifier) = ex.get_string(EXTH_ASIN)
            && !identifier.is_empty()
        {
            book.metadata.identifier = Some(identifier);
        }
    }
}

/// Extracts image records from the PDB and adds them to the Book.
/// Returns a vector of image href paths indexed by image record number (0-based).
fn extract_images_with_paths(
    pdb: &PdbFile,
    book: &mut Book,
    mobi: Option<&MobiHeader>,
) -> Vec<String> {
    let first_image = mobi
        .map(|h| h.first_image_index)
        .filter(|&idx| idx != NULL_INDEX)
        .unwrap_or(u32::MAX) as usize;

    let mut image_paths = Vec::new();

    if first_image >= pdb.record_count() {
        return image_paths;
    }

    let mut image_index = 0u32;
    const MAX_IMAGES: u32 = 100_000;

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
        image_paths.push(href);

        image_index = match image_index.checked_add(1) {
            Some(v) if v <= MAX_IMAGES => v,
            _ => break,
        };
    }

    image_paths
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
    } else if data.len() >= 2 && &data[0..2] == b"BM" {
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
    let bytes = html.as_bytes();
    let needle = b"<mbp:pagebreak";
    let mut parts = Vec::new();
    let mut last = 0;

    let mut search_from = 0;
    while let Some(offset) = text_utils::find_case_insensitive(&bytes[search_from..], needle) {
        let idx = search_from + offset;
        if idx > last {
            parts.push(&html[last..idx]);
        }
        // Find the end of this tag.
        if let Some(end) = html[idx..].find('>') {
            last = idx + end + 1;
        } else {
            last = idx + needle.len();
        }
        search_from = last;
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
    let bytes = html.as_bytes();

    for (open_tag, close_needle) in [
        (b"<h1" as &[u8], b"</h1" as &[u8]),
        (b"<h2", b"</h2"),
        (b"<h3", b"</h3"),
    ] {
        if let Some(start_idx) = text_utils::find_case_insensitive(bytes, open_tag) {
            // Find end of opening tag.
            let content_start = html[start_idx..].find('>')? + start_idx + 1;
            // Find closing tag.
            let content_end =
                text_utils::find_case_insensitive(&bytes[content_start..], close_needle)?
                    + content_start;

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
    crate::formats::common::text_utils::strip_tags(html).into_owned()
}

/// Decodes CP-1252 bytes to a UTF-8 string.
fn decode_cp1252(data: &[u8]) -> String {
    crate::formats::common::text_utils::decode_cp1252(data)
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
        let mut compressor = palmdoc::PalmDocCompressor::new();
        let mut offset = 0;
        while offset < text_bytes.len() {
            let end = (offset + palmdoc::RECORD_SIZE).min(text_bytes.len());
            let chunk = &text_bytes[offset..end];
            text_records.push(compressor.compress_record(chunk));
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
        let mobi_data = build_test_mobi(
            "Test Book",
            "<html><body><p>Hello MOBI</p></body></html>",
            &["Test Author"],
        );

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

    #[test]
    fn split_on_pagebreaks_case_insensitive() {
        let html = "part1<MBP:pagebreak />part2<Mbp:Pagebreak/>part3";
        let parts = split_on_pagebreaks(html);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "part1");
        assert_eq!(parts[1], "part2");
        assert_eq!(parts[2], "part3");
    }

    #[test]
    fn extract_heading_case_insensitive() {
        let html = "<H1>Title Here</H1><p>Content</p>";
        assert_eq!(extract_first_heading(html), Some("Title Here".into()));
    }

    #[test]
    fn extract_heading_mixed_case_h2() {
        let html = "<p>Intro</p><H2>Second Level</H2><p>More</p>";
        assert_eq!(extract_first_heading(html), Some("Second Level".into()));
    }

    #[test]
    fn detect_bmp_with_nonzero_file_size() {
        // BMP files start with "BM" followed by a 4-byte little-endian file size.
        // The old check required bytes 2-3 to be 0x00, which fails for real BMP files.
        let data = b"BM\x36\x04\x00\x00"; // "BM" + file size 1078 in LE
        let (ext, mime) = detect_image_type(data);
        assert_eq!(ext, "bmp");
        assert_eq!(mime, "image/bmp");
    }

    #[test]
    fn detect_bmp_minimal() {
        // Minimal 2-byte BM signature should be enough.
        let data = b"BM";
        let (ext, mime) = detect_image_type(data);
        assert_eq!(ext, "bmp");
        assert_eq!(mime, "image/bmp");
    }

    // --- kindle:embed base-32 decoder tests ---

    #[test]
    fn decode_kindle_base32_zero() {
        assert_eq!(decode_kindle_base32("0000"), Some(0));
    }

    #[test]
    fn decode_kindle_base32_one() {
        assert_eq!(decode_kindle_base32("0001"), Some(1));
    }

    #[test]
    fn decode_kindle_base32_004i() {
        // 0*32^3 + 0*32^2 + 4*32 + 18 = 128 + 18 = 146
        assert_eq!(decode_kindle_base32("004I"), Some(146));
    }

    #[test]
    fn decode_kindle_base32_004t() {
        // 0*32^3 + 0*32^2 + 4*32 + 29 = 128 + 29 = 157
        assert_eq!(decode_kindle_base32("004T"), Some(157));
    }

    #[test]
    fn decode_kindle_base32_000f() {
        // F = 15
        assert_eq!(decode_kindle_base32("000F"), Some(15));
    }

    #[test]
    fn decode_kindle_base32_001t() {
        // 0*32^3 + 0*32^2 + 1*32 + 29 = 61
        assert_eq!(decode_kindle_base32("001T"), Some(61));
    }

    #[test]
    fn decode_kindle_base32_case_insensitive() {
        assert_eq!(decode_kindle_base32("004i"), Some(146));
        assert_eq!(decode_kindle_base32("004I"), Some(146));
    }

    #[test]
    fn decode_kindle_base32_invalid_char() {
        // 'W' is out of the 0-9,A-V range
        assert_eq!(decode_kindle_base32("00W0"), None);
    }

    #[test]
    fn decode_kindle_base32_empty() {
        assert_eq!(decode_kindle_base32(""), Some(0));
    }

    // --- FDST parsing tests ---

    #[test]
    fn parse_fdst_synthetic() {
        // Build a synthetic FDST record.
        let mut fdst_data = vec![0u8; 12 + 3 * 8]; // header + 3 flows
        fdst_data[..4].copy_from_slice(b"FDST");
        write_u32_be(&mut fdst_data, 8, 3); // 3 flows
        // Flow 0: 0-1000
        write_u32_be(&mut fdst_data, 12, 0);
        write_u32_be(&mut fdst_data, 16, 1000);
        // Flow 1: 1000-1500
        write_u32_be(&mut fdst_data, 20, 1000);
        write_u32_be(&mut fdst_data, 24, 1500);
        // Flow 2: 1500-2000
        write_u32_be(&mut fdst_data, 28, 1500);
        write_u32_be(&mut fdst_data, 32, 2000);

        // Build a PDB with just this one record.
        let pdb_data = crate::formats::common::palm_db::build_pdb_header(
            "test", b"BOOK", b"MOBI", 1, &[88], // offset after header
        );
        let mut full_data = pdb_data;
        full_data.extend_from_slice(&fdst_data);

        let pdb = PdbFile::parse(full_data).unwrap();
        let entries = parse_fdst(&pdb, 0).unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].start, 0);
        assert_eq!(entries[0].end, 1000);
        assert_eq!(entries[1].start, 1000);
        assert_eq!(entries[1].end, 1500);
        assert_eq!(entries[2].start, 1500);
        assert_eq!(entries[2].end, 2000);
    }

    #[test]
    fn parse_fdst_invalid_magic() {
        let mut data = vec![0u8; 20];
        data[..4].copy_from_slice(b"NOPE");
        write_u32_be(&mut data, 8, 1);
        write_u32_be(&mut data, 12, 0);
        write_u32_be(&mut data, 16, 100);

        let pdb_data = crate::formats::common::palm_db::build_pdb_header(
            "test", b"BOOK", b"MOBI", 1, &[88],
        );
        let mut full_data = pdb_data;
        full_data.extend_from_slice(&data);

        let pdb = PdbFile::parse(full_data).unwrap();
        assert!(parse_fdst(&pdb, 0).is_none());
    }

    // --- kindle: reference resolution tests ---

    #[test]
    fn resolve_kindle_embed_basic() {
        let image_paths = vec![
            "images/0.jpg".to_string(),
            "images/1.png".to_string(),
        ];
        let flow_paths: Vec<Option<String>> = vec![];

        // kindle:embed:0001 is 1-based, so index 1 maps to image_paths[0]
        let html = r#"<img src="kindle:embed:0001?mime=image/jpeg">"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths);
        assert_eq!(result, r#"<img src="images/0.jpg">"#);
    }

    #[test]
    fn resolve_kindle_embed_second_image() {
        let image_paths = vec![
            "images/0.jpg".to_string(),
            "images/1.png".to_string(),
        ];
        let flow_paths: Vec<Option<String>> = vec![];

        // kindle:embed:0002 (1-based) maps to image_paths[1]
        let html = r#"<img src="kindle:embed:0002?mime=image/png">"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths);
        assert_eq!(result, r#"<img src="images/1.png">"#);
    }

    #[test]
    fn resolve_kindle_embed_no_query() {
        let image_paths = vec!["images/0.jpg".to_string()];
        let flow_paths: Vec<Option<String>> = vec![];

        // kindle:embed:0001 (1-based) → image_paths[0]
        let html = r#"<img src="kindle:embed:0001">"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths);
        assert_eq!(result, r#"<img src="images/0.jpg">"#);
    }

    #[test]
    fn resolve_kindle_embed_zero_left_as_is() {
        // kindle:embed:0000 decodes to 0, which is invalid for 1-based indexing
        let image_paths = vec!["images/0.jpg".to_string()];
        let flow_paths: Vec<Option<String>> = vec![];

        let html = r#"<img src="kindle:embed:0000?mime=image/jpeg">"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths);
        assert_eq!(result, html);
    }

    #[test]
    fn resolve_kindle_flow_basic() {
        let image_paths: Vec<String> = vec![];
        let flow_paths = vec![
            None, // flow 0 = main content
            Some("flows/flow_1.css".to_string()),
        ];

        let html = r#"<link href="kindle:flow:0001?mime=text/css">"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths);
        assert_eq!(result, r#"<link href="flows/flow_1.css">"#);
    }

    #[test]
    fn resolve_kindle_mixed_references() {
        let image_paths = vec![
            "images/0.jpg".to_string(),
            "images/1.png".to_string(),
        ];
        let flow_paths = vec![
            None,
            Some("flows/flow_1.css".to_string()),
            Some("flows/flow_2.css".to_string()),
        ];

        // kindle:embed:0002 (1-based) → image_paths[1] = images/1.png
        let html = concat!(
            r#"<link href="kindle:flow:0001?mime=text/css"/>"#,
            r#"<img src="kindle:embed:0002?mime=image/png"/>"#,
            r#"<link href="kindle:flow:0002?mime=text/css"/>"#,
        );
        let result = resolve_kindle_references(html, &image_paths, &flow_paths);
        assert_eq!(
            result,
            concat!(
                r#"<link href="flows/flow_1.css"/>"#,
                r#"<img src="images/1.png"/>"#,
                r#"<link href="flows/flow_2.css"/>"#,
            )
        );
    }

    #[test]
    fn resolve_kindle_out_of_range_left_as_is() {
        let image_paths = vec!["images/0.jpg".to_string()];
        let flow_paths: Vec<Option<String>> = vec![];

        // index 9999 is way out of range
        let html = r#"<img src="kindle:embed:009N?mime=image/jpeg">"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths);
        // Should be left unchanged since index is out of range
        assert_eq!(result, html);
    }

    #[test]
    fn resolve_kindle_no_references() {
        let image_paths: Vec<String> = vec![];
        let flow_paths: Vec<Option<String>> = vec![];

        let html = "<p>No kindle references here</p>";
        let result = resolve_kindle_references(html, &image_paths, &flow_paths);
        assert_eq!(result, html);
    }

    // --- Flow type detection tests ---

    #[test]
    fn detect_flow_type_css() {
        let data = b".class { color: red; }";
        assert_eq!(detect_flow_type(data), ("css", "text/css"));
    }

    #[test]
    fn detect_flow_type_svg() {
        let data = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><rect/></svg>";
        assert_eq!(detect_flow_type(data), ("svg", "image/svg+xml"));
    }

    #[test]
    fn detect_flow_type_svg_with_whitespace() {
        let data = b"  \n<svg><circle/></svg>";
        assert_eq!(detect_flow_type(data), ("svg", "image/svg+xml"));
    }
}
