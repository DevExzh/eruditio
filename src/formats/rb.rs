//! RB (RocketBook) ebook reader.
//!
//! Supports reading NuvoMedia RocketBook (.rb) files.
//! The format consists of a fixed header, a flat table of contents,
//! and concatenated data entries (HTML pages, images, and an info page).

use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use flate2::{Compress, Decompress};
use ahash::AHashMap as HashMap;
use std::io::{Read, Write};

/// RB ebook format reader.
#[derive(Default)]
pub struct RbReader;

impl RbReader {
    pub fn new() -> Self {
        Self
    }
}

/// Magic bytes at the start of every RB file.
const RB_MAGIC: [u8; 4] = [0xB0, 0x0C, 0xB0, 0x0C];

/// Minimum header size (fixed portion before TOC).
const HEADER_SIZE: usize = 0x128;

/// TOC entry flags.
const FLAG_RAW: u32 = 0;
const FLAG_ENCRYPTED: u32 = 1;
const FLAG_INFO: u32 = 2;
const FLAG_DEFLATED: u32 = 8;

/// Size of each TOC entry (32-byte name + 4 length + 4 offset + 4 flags).
const TOC_ENTRY_SIZE: usize = 44;

/// A single entry from the RB table of contents.
struct TocEntry {
    name: String,
    data_length: u32,
    data_offset: u32,
    flags: u32,
}

impl FormatReader for RbReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;

        if data.len() < HEADER_SIZE {
            return Err(EruditioError::Format("RB file too short for header".into()));
        }

        // Validate magic bytes.
        if data[0..4] != RB_MAGIC {
            return Err(EruditioError::Format(
                "Invalid RB magic bytes (expected B0 0C B0 0C)".into(),
            ));
        }

        let toc_offset = read_u32_le(&data, 0x18) as usize;
        if toc_offset + 4 > data.len() {
            return Err(EruditioError::Format(
                "RB TOC offset past end of file".into(),
            ));
        }

        // Parse TOC.
        let entries = parse_toc(&data, toc_offset)?;

        // Extract info page for metadata.
        let info = extract_info_page(&data, &entries)?;

        // Build book.
        let mut book = Book::new();
        book.metadata.title = info.get("TITLE").cloned();
        book.metadata.authors = info
            .get("AUTHOR")
            .map(|a| vec![a.clone()])
            .unwrap_or_default();
        if let Some(publisher) = info.get("PUBLISHER") {
            book.metadata.publisher = Some(publisher.clone());
        }

        // Find the root HTML page name from the BODY key.
        let body_name = info.get("BODY").cloned().unwrap_or_default();

        // Collect HTML pages in TOC order; put the body page first.
        let mut html_entries: Vec<&TocEntry> = entries
            .iter()
            .filter(|e| {
                e.flags == FLAG_DEFLATED || (e.flags == FLAG_RAW && e.name.ends_with(".html"))
            })
            .collect();

        // Sort: body page first, then remaining in original order.
        html_entries.sort_by(|a, b| {
            let a_is_body = a.name == body_name;
            let b_is_body = b.name == body_name;
            b_is_body.cmp(&a_is_body)
        });

        for (idx, entry) in html_entries.iter().enumerate() {
            let content = decompress_entry(&data, entry)?;
            let html = crate::formats::common::text_utils::bytes_to_string(&content);

            book.add_chapter(Chapter {
                title: if entry.name == body_name {
                    book.metadata.title.clone()
                } else {
                    Some(entry.name.clone())
                },
                content: html,
                id: Some(format!("rb_page_{}", idx)),
            });
        }

        if book.chapter_count() == 0 {
            return Err(EruditioError::Format(
                "RB file contains no readable content".into(),
            ));
        }

        Ok(book)
    }
}

/// RB (RocketBook) format writer.
#[derive(Default)]
pub struct RbWriter;

impl RbWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for RbWriter {
    fn write_book(&self, book: &Book, output: &mut dyn Write) -> Result<()> {
        // Build the info page.
        let title = book.metadata.title.as_deref().unwrap_or("Untitled");
        let author = book
            .metadata
            .authors
            .first()
            .map(|s| s.as_str())
            .unwrap_or("");
        let body_name = "index.html";

        let mut info_text = format!("TYPE=2\nTITLE={}\n", title);
        if !author.is_empty() {
            info_text.push_str(&format!("AUTHOR={}\n", author));
        }
        if let Some(publisher) = &book.metadata.publisher {
            info_text.push_str(&format!("PUBLISHER={}\n", publisher));
        }
        info_text.push_str(&format!("BODY={}\n", body_name));

        let info_bytes = info_text.into_bytes();

        // Build HTML content from chapters.
        let mut html = String::from("<html><body>\n");
        for chapter in book.chapter_views() {
            if let Some(title) = chapter.title {
                html.push_str(&format!("<h2>{}</h2>\n", title));
            }
            html.push_str(chapter.content);
            html.push('\n');
        }
        html.push_str("</body></html>");
        let html_bytes = html.into_bytes();

        // Collect image resources.
        let images: Vec<(&str, &[u8])> = book
            .resources()
            .iter()
            .filter(|r| r.media_type.starts_with("image/"))
            .map(|r| (r.href, r.data))
            .collect();

        // Build entries: info (raw), html (deflated), images (raw).
        struct RbEntry {
            name: String,
            data: Vec<u8>,
            flags: u32,
        }

        let mut entries = Vec::new();

        // Info entry (raw, flag=2).
        entries.push(RbEntry {
            name: "info".into(),
            data: info_bytes,
            flags: FLAG_INFO,
        });

        // HTML entry (deflated, flag=8).
        let compressed_html = compress_chunked(&html_bytes)?;
        entries.push(RbEntry {
            name: body_name.into(),
            data: compressed_html,
            flags: FLAG_DEFLATED,
        });

        // Image entries (raw, flag=0).
        for (href, img_data) in &images {
            let name = href.rsplit('/').next().unwrap_or(href);
            // Truncate name to 31 chars max.
            let name = if name.len() > 31 { &name[..31] } else { name };
            entries.push(RbEntry {
                name: name.to_string(),
                data: img_data.to_vec(),
                flags: FLAG_RAW,
            });
        }

        // Calculate layout.
        let toc_offset = HEADER_SIZE;
        let toc_size = 4 + entries.len() * TOC_ENTRY_SIZE;
        let data_start = toc_offset + toc_size;

        let mut entry_offsets = Vec::new();
        let mut pos = data_start;
        for entry in &entries {
            entry_offsets.push(pos as u32);
            pos += entry.data.len();
        }
        let total_size = pos;

        // Build file buffer.
        const MAX_OUTPUT_SIZE: usize = 512 * 1024 * 1024; // 512 MB
        if total_size > MAX_OUTPUT_SIZE {
            return Err(EruditioError::Format(
                "RB output exceeds maximum allowed size".into(),
            ));
        }
        let mut file = vec![0u8; total_size];

        // Write header.
        file[0..4].copy_from_slice(&RB_MAGIC);
        write_u16_le_buf(&mut file, 4, 2); // version
        file[6..10].copy_from_slice(b"NUVO");
        write_u32_le_buf(&mut file, 0x18, toc_offset as u32);
        write_u32_le_buf(&mut file, 0x1C, total_size as u32);

        // Write TOC.
        write_u32_le_buf(&mut file, toc_offset, entries.len() as u32);
        for (i, entry) in entries.iter().enumerate() {
            let base = toc_offset + 4 + i * TOC_ENTRY_SIZE;
            let name_bytes = entry.name.as_bytes();
            let copy_len = name_bytes.len().min(32);
            file[base..base + copy_len].copy_from_slice(&name_bytes[..copy_len]);
            write_u32_le_buf(&mut file, base + 32, entry.data.len() as u32);
            write_u32_le_buf(&mut file, base + 36, entry_offsets[i]);
            write_u32_le_buf(&mut file, base + 40, entry.flags);
        }

        // Write entry data.
        for (i, entry) in entries.iter().enumerate() {
            let off = entry_offsets[i] as usize;
            file[off..off + entry.data.len()].copy_from_slice(&entry.data);
        }

        output.write_all(&file)?;
        Ok(())
    }
}

/// Compresses data using chunked zlib (wbits=13) format matching RB spec.
///
/// Format: u32 chunk_count, u32 uncompressed_size,
///         chunk_count x u32 compressed_sizes,
///         compressed_chunks...
fn compress_chunked(data: &[u8]) -> Result<Vec<u8>> {
    let chunk_size = 4096usize;
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    let mut offset = 0;

    while offset < data.len() {
        let end = (offset + chunk_size).min(data.len());
        let chunk = &data[offset..end];
        let compressed = deflate_wbits13(chunk)?;
        chunks.push(compressed);
        offset = end;
    }

    if chunks.is_empty() {
        chunks.push(Vec::new());
    }

    // Build output: header + sizes + compressed data.
    let header_size = 8 + chunks.len() * 4;
    let total_compressed: usize = chunks.iter().map(|c| c.len()).sum();
    let mut result = vec![0u8; header_size + total_compressed];

    // Chunk count.
    result[0..4].copy_from_slice(&(chunks.len() as u32).to_le_bytes());
    // Uncompressed size.
    result[4..8].copy_from_slice(&(data.len() as u32).to_le_bytes());
    // Chunk sizes.
    for (i, chunk) in chunks.iter().enumerate() {
        let off = 8 + i * 4;
        result[off..off + 4].copy_from_slice(&(chunk.len() as u32).to_le_bytes());
    }
    // Compressed data.
    let mut pos = header_size;
    for chunk in &chunks {
        result[pos..pos + chunk.len()].copy_from_slice(chunk);
        pos += chunk.len();
    }

    Ok(result)
}

/// Compresses data with raw deflate (wbits=13, no zlib/gzip wrapper).
fn deflate_wbits13(data: &[u8]) -> Result<Vec<u8>> {
    let mut compress = Compress::new_with_window_bits(flate2::Compression::default(), false, 13);
    let mut output = vec![0u8; data.len() + 256];

    let status = compress
        .compress(data, &mut output, flate2::FlushCompress::Finish)
        .map_err(|e| EruditioError::Compression(format!("RB zlib compression error: {}", e)))?;

    match status {
        flate2::Status::StreamEnd => {
            output.truncate(compress.total_out() as usize);
            Ok(output)
        },
        _ => {
            // Need more space — retry with larger buffer.
            let mut output2 = vec![0u8; data.len() * 2 + 512];
            let mut compress2 =
                Compress::new_with_window_bits(flate2::Compression::default(), false, 13);
            compress2
                .compress(data, &mut output2, flate2::FlushCompress::Finish)
                .map_err(|e| {
                    EruditioError::Compression(format!("RB zlib compression error: {}", e))
                })?;
            output2.truncate(compress2.total_out() as usize);
            Ok(output2)
        },
    }
}

fn write_u16_le_buf(buf: &mut [u8], offset: usize, val: u16) {
    buf[offset..offset + 2].copy_from_slice(&val.to_le_bytes());
}

fn write_u32_le_buf(buf: &mut [u8], offset: usize, val: u32) {
    buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
}

/// Parses the table of contents from the given offset.
fn parse_toc(data: &[u8], offset: usize) -> Result<Vec<TocEntry>> {
    if offset + 4 > data.len() {
        return Err(EruditioError::Format("TOC offset out of bounds".into()));
    }

    let page_count = read_u32_le(data, offset) as usize;
    let entries_start = offset + 4;

    let entries_end = page_count
        .checked_mul(TOC_ENTRY_SIZE)
        .and_then(|s| entries_start.checked_add(s))
        .ok_or_else(|| EruditioError::Format("RB TOC entries size overflow".into()))?;
    if entries_end > data.len() {
        return Err(EruditioError::Format(
            "TOC entries extend past end of file".into(),
        ));
    }

    // Cap allocation to prevent DoS from malformed headers.
    let max_entries = (data.len() - entries_start) / TOC_ENTRY_SIZE;
    let mut entries = Vec::with_capacity(page_count.min(max_entries));
    for i in 0..page_count {
        let base = entries_start + i * TOC_ENTRY_SIZE;
        let name_bytes = &data[base..base + 32];
        let name = String::from_utf8_lossy(
            &name_bytes[..name_bytes.iter().position(|&b| b == 0).unwrap_or(32)],
        )
        .into_owned();

        entries.push(TocEntry {
            name,
            data_length: read_u32_le(data, base + 32),
            data_offset: read_u32_le(data, base + 36),
            flags: read_u32_le(data, base + 40),
        });
    }

    Ok(entries)
}

/// Extracts and parses the info page (flag=2) into key-value pairs.
fn extract_info_page(data: &[u8], entries: &[TocEntry]) -> Result<HashMap<String, String>> {
    let mut info = HashMap::new();

    let info_entry = entries.iter().find(|e| e.flags == FLAG_INFO);
    let Some(entry) = info_entry else {
        return Ok(info);
    };

    let content = decompress_entry(data, entry)?;
    let text = String::from_utf8_lossy(&content);

    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            info.insert(key.trim().to_string(), value.trim().to_string());
        }
    }

    Ok(info)
}

/// Decompresses (or reads raw) a TOC entry's data.
fn decompress_entry(data: &[u8], entry: &TocEntry) -> Result<Vec<u8>> {
    let offset = entry.data_offset as usize;
    let length = entry.data_length as usize;

    let end = offset.checked_add(length).ok_or_else(|| {
        EruditioError::Format(format!(
            "RB entry '{}' data offset+length overflow",
            entry.name
        ))
    })?;
    if end > data.len() {
        return Err(EruditioError::Format(format!(
            "RB entry '{}' data extends past end of file",
            entry.name
        )));
    }

    match entry.flags {
        FLAG_RAW | FLAG_INFO => Ok(data[offset..offset + length].to_vec()),
        FLAG_DEFLATED => decompress_chunked(&data[offset..offset + length], &entry.name),
        FLAG_ENCRYPTED => Err(EruditioError::Unsupported(format!(
            "Encrypted RB entry '{}' not supported",
            entry.name
        ))),
        other => Err(EruditioError::Format(format!(
            "Unknown RB entry flag {} for '{}'",
            other, entry.name
        ))),
    }
}

/// Decompresses chunked zlib data (wbits=13).
///
/// Format: u32 chunk_count, u32 uncompressed_size,
///         chunk_count x u32 compressed_chunk_sizes,
///         chunk_count x compressed_data
fn decompress_chunked(data: &[u8], name: &str) -> Result<Vec<u8>> {
    if data.len() < 8 {
        return Err(EruditioError::Format(format!(
            "RB deflated entry '{}' too short for chunk header",
            name
        )));
    }

    let chunk_count = read_u32_le(data, 0) as usize;
    let uncompressed_size = read_u32_le(data, 4) as usize;

    let sizes_start: usize = 8;
    let sizes_end = chunk_count
        .checked_mul(4)
        .and_then(|s| sizes_start.checked_add(s))
        .ok_or_else(|| {
            EruditioError::Format(format!("RB deflated entry '{}' chunk sizes overflow", name))
        })?;
    if sizes_end > data.len() {
        return Err(EruditioError::Format(format!(
            "RB deflated entry '{}' chunk sizes extend past data",
            name
        )));
    }

    // Read compressed chunk sizes.
    let chunk_sizes: Vec<usize> = (0..chunk_count)
        .map(|i| read_u32_le(data, sizes_start + i * 4) as usize)
        .collect();

    // Cap allocation to prevent DoS from crafted headers claiming huge sizes.
    const MAX_PREALLOC: usize = 64 * 1024 * 1024;
    let mut result = Vec::with_capacity(uncompressed_size.min(MAX_PREALLOC));
    let mut pos = sizes_end;

    for (i, &csize) in chunk_sizes.iter().enumerate() {
        if pos + csize > data.len() {
            return Err(EruditioError::Format(format!(
                "RB deflated entry '{}' chunk {} extends past data",
                name, i
            )));
        }

        let compressed = &data[pos..pos + csize];
        let decompressed = zlib_decompress_wbits(compressed, 13)?;
        result.extend_from_slice(&decompressed);
        pos += csize;
    }

    Ok(result)
}

/// Decompresses raw deflate data with the specified window bits.
fn zlib_decompress_wbits(data: &[u8], wbits: u8) -> Result<Vec<u8>> {
    let mut decompress = Decompress::new_with_window_bits(false, wbits);
    // Pre-allocate a reasonable buffer (4x compressed size or 4KB minimum).
    let initial_cap = (data.len() * 4).max(4096);
    let mut output = vec![0u8; initial_cap];
    // Cap total decompressed output to prevent zip-bomb DoS.
    const MAX_OUTPUT: usize = 256 * 1024 * 1024;

    loop {
        let in_before = decompress.total_in() as usize;
        let out_before = decompress.total_out() as usize;

        let status = decompress
            .decompress(
                &data[in_before..],
                &mut output[out_before..],
                flate2::FlushDecompress::Finish,
            )
            .map_err(|e| {
                EruditioError::Compression(format!("RB zlib decompression error: {}", e))
            })?;

        match status {
            flate2::Status::StreamEnd => break,
            flate2::Status::Ok | flate2::Status::BufError => {
                if output.len() >= MAX_OUTPUT {
                    return Err(EruditioError::Compression(
                        "RB decompression exceeded 256 MB limit".into(),
                    ));
                }
                // Need more output space.
                output.resize((output.len() * 2).min(MAX_OUTPUT), 0);
            },
        }
    }

    output.truncate(decompress.total_out() as usize);
    Ok(output)
}

/// Reads a little-endian u32.
fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compress;
    use std::io::Cursor;

    fn write_u32_le(buf: &mut [u8], offset: usize, val: u32) {
        buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
    }

    fn write_u16_le(buf: &mut [u8], offset: usize, val: u16) {
        buf[offset..offset + 2].copy_from_slice(&val.to_le_bytes());
    }

    /// Compress data with raw deflate (wbits=13, no zlib header).
    fn deflate_wbits13(data: &[u8]) -> Vec<u8> {
        let mut compress = Compress::new_with_window_bits(flate2::Compression::best(), false, 13);
        let mut output = vec![0u8; data.len() + 256];
        let status = compress
            .compress(data, &mut output, flate2::FlushCompress::Finish)
            .unwrap();
        assert_eq!(status, flate2::Status::StreamEnd);
        output.truncate(compress.total_out() as usize);
        output
    }

    /// Builds a minimal RB file for testing.
    fn build_rb_file(pages: &[(&str, &[u8], u32)]) -> Vec<u8> {
        // pages: (name, content, flags)
        // For deflated pages, we'll compress the content.

        let total_entries = pages.len();
        let toc_offset = HEADER_SIZE;
        let toc_size = 4 + total_entries * TOC_ENTRY_SIZE;

        // Prepare entry data (compressed where needed).
        let mut entry_data: Vec<Vec<u8>> = Vec::new();
        for &(_, content, flags) in pages {
            if flags == FLAG_DEFLATED {
                // Build chunked compressed format.
                let compressed_chunk = deflate_wbits13(content);
                let chunk_count = 1u32;
                let mut chunk_data = vec![0u8; 8 + 4 + compressed_chunk.len()];
                write_u32_le(&mut chunk_data, 0, chunk_count);
                write_u32_le(&mut chunk_data, 4, content.len() as u32);
                write_u32_le(&mut chunk_data, 8, compressed_chunk.len() as u32);
                chunk_data[12..12 + compressed_chunk.len()].copy_from_slice(&compressed_chunk);
                chunk_data.truncate(12 + compressed_chunk.len());
                entry_data.push(chunk_data);
            } else {
                entry_data.push(content.to_vec());
            }
        }

        // Calculate offsets.
        let data_start = toc_offset + toc_size;
        let mut offsets = Vec::new();
        let mut pos = data_start;
        for ed in &entry_data {
            offsets.push(pos as u32);
            pos += ed.len();
        }
        let total_file_size = pos;

        // Build header.
        let mut file = vec![0u8; total_file_size];
        file[0..4].copy_from_slice(&RB_MAGIC);
        write_u16_le(&mut file, 4, 2); // version
        file[6..10].copy_from_slice(b"NUVO");
        write_u32_le(&mut file, 0x18, toc_offset as u32);
        write_u32_le(&mut file, 0x1C, total_file_size as u32);

        // Build TOC.
        let toc_base = toc_offset;
        write_u32_le(&mut file, toc_base, total_entries as u32);
        for (i, &(name, _, flags)) in pages.iter().enumerate() {
            let entry_base = toc_base + 4 + i * TOC_ENTRY_SIZE;
            let name_bytes = name.as_bytes();
            let copy_len = name_bytes.len().min(32);
            file[entry_base..entry_base + copy_len].copy_from_slice(&name_bytes[..copy_len]);
            write_u32_le(&mut file, entry_base + 32, entry_data[i].len() as u32);
            write_u32_le(&mut file, entry_base + 36, offsets[i]);
            write_u32_le(&mut file, entry_base + 40, flags);
        }

        // Write entry data.
        for (i, ed) in entry_data.iter().enumerate() {
            let off = offsets[i] as usize;
            file[off..off + ed.len()].copy_from_slice(ed);
        }

        file
    }

    #[test]
    fn reads_uncompressed_rb() {
        let html = b"<html><body><p>Hello, RocketBook!</p></body></html>";
        let info = b"TITLE=Test Book\nAUTHOR=Test Author\nBODY=page1.html";

        let data = build_rb_file(&[("info", info, FLAG_INFO), ("page1.html", html, FLAG_RAW)]);

        let mut cursor = Cursor::new(data);
        let book = RbReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Test Book"));
        assert_eq!(book.metadata.authors, vec!["Test Author"]);
        let content: String = book.chapter_views().iter().map(|c| c.content).collect();
        assert!(content.contains("Hello, RocketBook!"));
    }

    #[test]
    fn reads_compressed_rb() {
        let html = b"<html><body><p>Compressed content here!</p></body></html>";
        let info = b"TITLE=Compressed Book\nBODY=chapter.html";

        let data = build_rb_file(&[
            ("info", info, FLAG_INFO),
            ("chapter.html", html, FLAG_DEFLATED),
        ]);

        let mut cursor = Cursor::new(data);
        let book = RbReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Compressed Book"));
        let content: String = book.chapter_views().iter().map(|c| c.content).collect();
        assert!(content.contains("Compressed content"));
    }

    #[test]
    fn rejects_invalid_magic() {
        let mut data = vec![0u8; HEADER_SIZE + 4];
        data[0..4].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);

        let mut cursor = Cursor::new(data);
        let result = RbReader::new().read_book(&mut cursor);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("magic"));
    }

    #[test]
    fn rejects_too_short_file() {
        let data = vec![0xB0, 0x0C, 0xB0, 0x0C, 0x02, 0x00];
        let mut cursor = Cursor::new(data);
        let result = RbReader::new().read_book(&mut cursor);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too short"));
    }

    #[test]
    fn extracts_publisher_from_info() {
        let html = b"<html><body><p>Content</p></body></html>";
        let info = b"TITLE=Book\nAUTHOR=Author\nPUBLISHER=MyPress\nBODY=p.html";

        let data = build_rb_file(&[("info", info, FLAG_INFO), ("p.html", html, FLAG_RAW)]);

        let mut cursor = Cursor::new(data);
        let book = RbReader::new().read_book(&mut cursor).unwrap();
        assert_eq!(book.metadata.publisher.as_deref(), Some("MyPress"));
    }

    #[test]
    fn handles_missing_info_page() {
        let html = b"<html><body><p>No info page</p></body></html>";

        let data = build_rb_file(&[("page.html", html, FLAG_RAW)]);

        let mut cursor = Cursor::new(data);
        let book = RbReader::new().read_book(&mut cursor).unwrap();

        assert!(book.metadata.title.is_none());
        let content: String = book.chapter_views().iter().map(|c| c.content).collect();
        assert!(content.contains("No info page"));
    }

    #[test]
    fn rb_writer_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("RB Write Test".into());
        book.metadata.authors = vec!["Test Author".into()];
        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello RocketBook!</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        RbWriter::new().write_book(&book, &mut output).unwrap();

        // Read it back.
        let mut cursor = Cursor::new(output);
        let decoded = RbReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("RB Write Test"));
        assert_eq!(decoded.metadata.authors, vec!["Test Author"]);
        let content: String = decoded
            .chapters()
            .iter()
            .map(|c| c.content.clone())
            .collect();
        assert!(content.contains("Hello RocketBook!"));
    }
}
