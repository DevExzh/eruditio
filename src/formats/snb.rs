//! SNB (Shanda Bambook) ebook reader and writer.
//!
//! Reads and writes Shanda Bambook `.snb` files — a custom archive format with
//! zlib-compressed VFAT, bz2-compressed plain streams, and XML content.

use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use crate::formats::common::text_utils::{
    ends_with_ascii_ci, push_escape_html, push_escape_xml, strip_tags_and_unescape,
};
use crate::formats::common::xml_utils;
use bzip2::Compression as BzCompression;
use bzip2::read::BzDecoder;
use bzip2::write::BzEncoder;
use flate2::Compression;
use flate2::bufread::ZlibDecoder;
use flate2::write::ZlibEncoder;
use quick_xml::Reader;
use quick_xml::events::Event;
use ahash::AHashMap as HashMap;
use std::io::{Read, Write};

/// SNB ebook format reader.
#[derive(Default)]
pub struct SnbReader;

impl SnbReader {
    pub fn new() -> Self {
        Self
    }
}

/// SNB magic bytes.
const SNB_MAGIC: &[u8; 8] = b"SNBP000B";

/// SNB header size.
const HEADER_SIZE: usize = 0x2C;

/// File attribute: plain/text (bz2-compressed in plain stream).
const ATTR_PLAIN: u32 = 0x41000000;

/// File attribute: binary (raw in binary stream).
const ATTR_BINARY: u32 = 0x01000000;

/// Block size for plain stream decompression.
const BLOCK_SIZE: usize = 0x8000; // 32 KB

/// Parsed SNB header.
struct SnbHeader {
    file_count: usize,
    vfat_size: usize,
    vfat_compressed: usize,
    bin_stream_size: usize,
    _plain_uncompressed: usize,
}

/// A virtual file entry from the VFAT.
struct VfatEntry {
    attr: u32,
    name: String,
    size: usize,
}

/// A chapter entry from the TOC.
struct TocChapter {
    src: String,
    title: String,
}

impl FormatReader for SnbReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        const MAX_SNB_FILE: u64 = 256 * 1024 * 1024; // 256 MB
        let mut data = Vec::new();
        reader.take(MAX_SNB_FILE).read_to_end(&mut data)?;

        if data.len() < HEADER_SIZE + 16 {
            return Err(EruditioError::Format("SNB file too short".into()));
        }

        // Validate magic.
        if &data[0..8] != SNB_MAGIC {
            return Err(EruditioError::Format(
                "Invalid SNB magic (expected SNBP000B)".into(),
            ));
        }

        let header = parse_header(&data)?;
        let entries = parse_vfat(&data, &header)?;

        // Locate stream regions.
        let bin_start = HEADER_SIZE + header.vfat_compressed;
        let plain_start = bin_start + header.bin_stream_size;

        // Parse tail to get file-to-block mapping.
        let tail = parse_tail(&data)?;

        // Build a file-extraction map.
        let files = extract_files(&data, &entries, &tail, bin_start, plain_start)?;

        // Parse metadata.
        let mut book = Book::new();

        if let Some(meta_xml) = files.get("snbf/book.snbf") {
            parse_book_metadata(
                &crate::formats::common::text_utils::bytes_to_cow_str(meta_xml),
                &mut book,
            );
        }

        // Parse TOC.
        let chapters = if let Some(toc_xml) = files.get("snbf/toc.snbf") {
            parse_toc(&crate::formats::common::text_utils::bytes_to_cow_str(
                toc_xml,
            ))
        } else {
            Vec::new()
        };

        // Build chapters from TOC + content files.
        if chapters.is_empty() {
            // No TOC — gather all .snbc files as chapters.
            let mut snbc_files: Vec<(&String, &Vec<u8>)> =
                files.iter().filter(|(k, _)| k.ends_with(".snbc")).collect();
            snbc_files.sort_by(|a, b| a.0.cmp(b.0));

            for (idx, (name, content)) in snbc_files.iter().enumerate() {
                let html = snbc_to_html(&crate::formats::common::text_utils::bytes_to_cow_str(
                    content,
                ));
                book.add_chapter(Chapter {
                    title: Some(name.to_string()),
                    content: html,
                    id: Some(format!("snb_ch_{}", idx)),
                });
            }
        } else {
            for (idx, ch) in chapters.iter().enumerate() {
                let content = files
                    .get(&ch.src)
                    .or_else(|| files.get(&format!("snbc/{}", ch.src)));

                let html = match content {
                    Some(data) => {
                        snbc_to_html(&crate::formats::common::text_utils::bytes_to_cow_str(data))
                    },
                    None => {
                        let mut s = String::with_capacity(7 + ch.title.len());
                        s.push_str("<p>");
                        push_escape_html(&mut s, &ch.title);
                        s.push_str("</p>");
                        s
                    },
                };

                book.add_chapter(Chapter {
                    title: Some(ch.title.clone()),
                    content: html,
                    id: Some(format!("snb_ch_{}", idx)),
                });
            }
        }

        if book.chapter_count() == 0 {
            book.add_chapter(Chapter {
                title: book.metadata.title.clone(),
                content: "<p></p>".into(),
                id: Some("snb_empty".into()),
            });
        }

        // Add image resources last — consume the HashMap to move data, avoiding clone.
        for (name, content) in files {
            let is_image = ends_with_ascii_ci(&name, ".jpg")
                || ends_with_ascii_ci(&name, ".jpeg")
                || ends_with_ascii_ci(&name, ".png")
                || ends_with_ascii_ci(&name, ".gif")
                || ends_with_ascii_ci(&name, ".bmp");
            if is_image {
                let media_type = if ends_with_ascii_ci(&name, ".png") {
                    "image/png"
                } else if ends_with_ascii_ci(&name, ".gif") {
                    "image/gif"
                } else if ends_with_ascii_ci(&name, ".bmp") {
                    "image/bmp"
                } else {
                    "image/jpeg"
                };
                let basename = name.rsplit('/').next().unwrap_or(&name);
                book.add_resource(basename, basename, content, media_type);
            }
        }

        Ok(book)
    }
}

/// Parses the SNB header.
fn parse_header(data: &[u8]) -> Result<SnbHeader> {
    // Validate i32 fields are non-negative before casting to usize,
    // since crafted files with negative values would wrap to huge usize values.
    let file_count = usize::try_from(read_i32_be(data, 0x14))
        .map_err(|_| EruditioError::Format("SNB: negative file_count".into()))?;
    let vfat_size = usize::try_from(read_i32_be(data, 0x18))
        .map_err(|_| EruditioError::Format("SNB: negative vfat_size".into()))?;
    let vfat_compressed = usize::try_from(read_i32_be(data, 0x1C))
        .map_err(|_| EruditioError::Format("SNB: negative vfat_compressed".into()))?;
    let bin_stream_size = usize::try_from(read_i32_be(data, 0x20))
        .map_err(|_| EruditioError::Format("SNB: negative bin_stream_size".into()))?;
    let _plain_uncompressed = usize::try_from(read_i32_be(data, 0x24))
        .map_err(|_| EruditioError::Format("SNB: negative plain_uncompressed".into()))?;
    Ok(SnbHeader {
        file_count,
        vfat_size,
        vfat_compressed,
        bin_stream_size,
        _plain_uncompressed,
    })
}

/// Decompresses and parses the VFAT (Virtual File Allocation Table).
fn parse_vfat(data: &[u8], header: &SnbHeader) -> Result<Vec<VfatEntry>> {
    let vfat_start = HEADER_SIZE;
    let vfat_end = vfat_start + header.vfat_compressed;

    if vfat_end > data.len() {
        return Err(EruditioError::Format(
            "SNB VFAT extends past end of file".into(),
        ));
    }

    let vfat_bytes = zlib_decompress(&data[vfat_start..vfat_end], header.vfat_size)?;

    let entry_size = 12;
    let entries_region = header.file_count * entry_size;

    if vfat_bytes.len() < entries_region {
        return Err(EruditioError::Format(
            "SNB VFAT too short for entries".into(),
        ));
    }

    // Parse entries. Cap allocation by available data to prevent DoS.
    let max_entries = vfat_bytes.len() / entry_size;
    let mut entries = Vec::with_capacity(header.file_count.min(max_entries));
    for i in 0..header.file_count {
        let base = i * entry_size;
        let attr = read_u32_be(&vfat_bytes, base);
        let name_offset = read_u32_be(&vfat_bytes, base + 4) as usize;
        let size = read_u32_be(&vfat_bytes, base + 8) as usize;

        // Read null-terminated filename from after the entry table.
        let name_start = entries_region + name_offset;
        let name = read_cstring(&vfat_bytes, name_start);

        entries.push(VfatEntry { attr, name, size });
    }

    Ok(entries)
}

/// Parsed tail block containing file-to-block mapping.
struct TailInfo {
    /// For each file: (block_index, content_offset_within_block).
    file_records: Vec<(usize, usize)>,
    /// Block offsets (relative to the start of each stream).
    block_offsets: Vec<usize>,
    /// Number of binary stream blocks.
    bin_block_count: usize,
}

/// Parses the tail block from the end of the file.
fn parse_tail(data: &[u8]) -> Result<TailInfo> {
    if data.len() < 16 {
        return Err(EruditioError::Format("SNB file too short for tail".into()));
    }

    let tail_footer = &data[data.len() - 16..];
    if &tail_footer[8..16] != SNB_MAGIC {
        return Err(EruditioError::Format("SNB tail magic mismatch".into()));
    }

    let tail_compressed_size = usize::try_from(read_i32_be(tail_footer, 0))
        .map_err(|_| EruditioError::Format("SNB: negative tail compressed size".into()))?;
    let tail_offset = usize::try_from(read_i32_be(tail_footer, 4))
        .map_err(|_| EruditioError::Format("SNB: negative tail offset".into()))?;

    if tail_offset + tail_compressed_size > data.len() {
        return Err(EruditioError::Format(
            "SNB tail block extends past end of file".into(),
        ));
    }

    let tail_data = zlib_decompress(
        &data[tail_offset..tail_offset + tail_compressed_size],
        0, // unknown uncompressed size
    )?;

    if tail_data.len() < 8 {
        return Err(EruditioError::Format("SNB tail block too short".into()));
    }

    // Tail structure: block offsets, then file records.
    // We need to figure out the layout. The tail contains:
    // - Total block count × i32 offsets (bin blocks + plain blocks)
    // - File count × (i32 block_index, i32 content_offset)
    //
    // We derive the counts from the file entries and header.
    // For simplicity, scan from the end: file records are at the end.

    // Read the header info to determine file_count.
    let file_count = usize::try_from(read_i32_be(data, 0x14)).unwrap_or(0);

    // File records are at the end of the tail: file_count × 8 bytes.
    let records_size = file_count * 8;
    if tail_data.len() < records_size {
        return Err(EruditioError::Format(
            "SNB tail too short for file records".into(),
        ));
    }

    let block_offsets_size = tail_data.len() - records_size;
    let total_blocks = block_offsets_size / 4;

    // Parse block offsets.
    let block_offsets: Vec<usize> = (0..total_blocks)
        .map(|i| usize::try_from(read_i32_be(&tail_data, i * 4)).unwrap_or(0))
        .collect();

    // Parse file records.
    let records_start = block_offsets_size;
    let file_records: Vec<(usize, usize)> = (0..file_count)
        .map(|i| {
            let base = records_start + i * 8;
            let block_idx = usize::try_from(read_i32_be(&tail_data, base)).unwrap_or(0);
            let content_off = usize::try_from(read_i32_be(&tail_data, base + 4)).unwrap_or(0);
            (block_idx, content_off)
        })
        .collect();

    // Determine bin_block_count from the binary stream size.
    let bin_stream_size = usize::try_from(read_i32_be(data, 0x20)).unwrap_or(0);
    let bin_block_count = if bin_stream_size > 0 {
        // Count how many block offsets fall within the binary stream range.
        block_offsets
            .iter()
            .take_while(|&&off| off < bin_stream_size)
            .count()
            .max(1)
    } else {
        0
    };

    Ok(TailInfo {
        file_records,
        block_offsets,
        bin_block_count,
    })
}

/// Extracts all files from the SNB container.
fn extract_files(
    data: &[u8],
    entries: &[VfatEntry],
    tail: &TailInfo,
    bin_start: usize,
    plain_start: usize,
) -> Result<HashMap<String, Vec<u8>>> {
    let mut files = HashMap::new();

    for (i, entry) in entries.iter().enumerate() {
        if i >= tail.file_records.len() {
            break;
        }

        let (block_idx, content_offset) = tail.file_records[i];

        let content = if entry.attr == ATTR_BINARY {
            // Binary file: read raw from binary stream.
            let offset = bin_start + content_offset;
            let end = (offset + entry.size).min(data.len());
            if offset < data.len() {
                data[offset..end].to_vec()
            } else {
                continue;
            }
        } else if entry.attr == ATTR_PLAIN {
            // Plain file: decompress from plain stream blocks.
            extract_plain_file(
                data,
                plain_start,
                tail,
                block_idx,
                content_offset,
                entry.size,
            )?
        } else {
            continue;
        };

        files.insert(entry.name.clone(), content);
    }

    Ok(files)
}

/// Extracts a file from the bz2-compressed plain stream.
fn extract_plain_file(
    data: &[u8],
    plain_start: usize,
    tail: &TailInfo,
    block_idx: usize,
    content_offset: usize,
    file_size: usize,
) -> Result<Vec<u8>> {
    // The plain stream blocks start after bin blocks in the offset table.
    let plain_block_base = tail.bin_block_count;

    let mut result = Vec::with_capacity(file_size.min(64 * 1024 * 1024));
    let mut remaining = file_size;
    let mut first_block = true;

    let mut current_block = block_idx;

    while remaining > 0 {
        let block_idx_in_offsets =
            plain_block_base + (current_block - plain_block_base.min(current_block));
        if block_idx_in_offsets >= tail.block_offsets.len() {
            break;
        }

        let block_offset = tail.block_offsets[block_idx_in_offsets];
        let abs_offset = plain_start + block_offset;

        if abs_offset >= data.len() {
            break;
        }

        // Determine the block's compressed size (distance to next block or end of stream).
        let next_offset = if block_idx_in_offsets + 1 < tail.block_offsets.len() {
            plain_start + tail.block_offsets[block_idx_in_offsets + 1]
        } else {
            // Use remaining data up to the tail.
            data.len() - 16
        };

        let block_data = &data[abs_offset..next_offset.min(data.len())];

        // Decompress the block (bz2 for blocks <= 32KB uncompressed).
        let decompressed = if block_data.len() >= BLOCK_SIZE {
            // Large block — likely uncompressed.
            block_data.to_vec()
        } else {
            bz2_decompress(block_data)?
        };

        let start = if first_block { content_offset } else { 0 };
        let avail = if start < decompressed.len() {
            &decompressed[start..]
        } else {
            break;
        };

        let take = remaining.min(avail.len());
        result.extend_from_slice(&avail[..take]);
        remaining -= take;
        first_block = false;
        current_block += 1;
    }

    Ok(result)
}

/// Parses the book.snbf metadata XML.
fn parse_book_metadata(xml: &str, book: &mut Book) {
    let mut reader = Reader::from_str(xml);
    let mut current_tag: &[u8] = b"";
    let mut text_buf = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                current_tag = match e.name().as_ref() {
                    b"name" => b"name",
                    b"author" => b"author",
                    b"language" => b"language",
                    b"publisher" => b"publisher",
                    b"abstract" => b"abstract",
                    _ => b"",
                };
            },
            Ok(Event::Text(ref e)) => {
                if current_tag.is_empty() {
                    continue;
                }
                text_buf.clear();
                xml_utils::push_text_bytes(&mut text_buf, e.as_ref());
                if text_buf.trim().is_empty() {
                    continue;
                }
                match current_tag {
                    b"name" => book.metadata.title = Some(text_buf.trim().to_string()),
                    b"author" => book.metadata.authors = vec![text_buf.trim().to_string()],
                    b"language" => book.metadata.language = Some(text_buf.trim().to_string()),
                    b"publisher" => book.metadata.publisher = Some(text_buf.trim().to_string()),
                    b"abstract" => book.metadata.description = Some(text_buf.trim().to_string()),
                    _ => {},
                }
            },
            Ok(Event::End(_)) => current_tag = b"",
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {},
        }
    }
}

/// Parses the toc.snbf TOC XML.
fn parse_toc(xml: &str) -> Vec<TocChapter> {
    let mut reader = Reader::from_str(xml);
    let mut chapters = Vec::new();
    let mut current_src = String::new();
    let mut in_chapter = false;
    let mut text_buf = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                if e.name().as_ref() == b"chapter" {
                    in_chapter = true;
                    if let Some(attr) = e.try_get_attribute(b"src").ok().flatten() {
                        current_src.clear();
                        xml_utils::push_text_bytes(&mut current_src, &attr.value);
                    }
                }
            },
            Ok(Event::Text(ref e)) if in_chapter => {
                text_buf.clear();
                xml_utils::push_text_bytes(&mut text_buf, e.as_ref());
                if !text_buf.trim().is_empty() {
                    chapters.push(TocChapter {
                        src: current_src.clone(),
                        title: text_buf.trim().to_string(),
                    });
                }
            },
            Ok(Event::End(ref e)) => {
                if e.name().as_ref() == b"chapter" {
                    in_chapter = false;
                    current_src.clear();
                }
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {},
        }
    }

    chapters
}

/// Converts SNBC chapter XML to HTML.
///
/// SNBC format:
/// ```xml
/// <snbc>
///   <head><title>Chapter Title</title></head>
///   <body>
///     <text>Paragraph text</text>
///     <img>image.jpg</img>
///   </body>
/// </snbc>
/// ```
fn snbc_to_html(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    let mut html = String::with_capacity(xml.len());
    let mut current_tag: &[u8] = b"";
    let mut in_body = false;
    let mut text_buf = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let name = e.name();
                let tag = name.as_ref();
                if tag == b"body" {
                    in_body = true;
                }
                current_tag = match tag {
                    b"text" => b"text",
                    b"img" => b"img",
                    b"body" => b"body",
                    _ => b"",
                };
            },
            Ok(Event::Text(ref e)) if in_body => {
                text_buf.clear();
                xml_utils::push_text_bytes(&mut text_buf, e.as_ref());
                if text_buf.trim().is_empty() {
                    continue;
                }
                match current_tag {
                    b"text" => {
                        html.push_str("<p>");
                        push_escape_html(&mut html, text_buf.trim());
                        html.push_str("</p>\n");
                    },
                    b"img" => {
                        html.push_str("<img src=\"");
                        push_escape_html(&mut html, text_buf.trim());
                        html.push_str("\" />\n");
                    },
                    _ => {},
                }
            },
            Ok(Event::End(ref e)) => {
                if e.name().as_ref() == b"body" {
                    in_body = false;
                }
                current_tag = b"";
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {},
        }
    }

    if html.is_empty() {
        html.push_str("<p></p>");
    }

    html
}

// -- Writer --

/// SNB (Shanda Bambook) format writer.
#[derive(Default)]
pub struct SnbWriter;

impl SnbWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for SnbWriter {
    fn write_book(&self, book: &Book, output: &mut dyn Write) -> Result<()> {
        // Build virtual files.
        let mut plain_files: Vec<(String, Vec<u8>)> = Vec::new();
        let mut binary_files: Vec<(String, Vec<u8>)> = Vec::new();

        // 1. Build book.snbf metadata XML.
        let title = book.metadata.title.as_deref().unwrap_or("Untitled");
        let author = book
            .metadata
            .authors
            .first()
            .map(|s| s.as_str())
            .unwrap_or("");
        let language = book.metadata.language.as_deref().unwrap_or("en");
        let publisher = book.metadata.publisher.as_deref().unwrap_or("");

        let mut meta_xml = String::with_capacity(256);
        meta_xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <book-snbf version=\"1.0\">\n  <head>\n\
             \x20   <name>");
        push_escape_xml(&mut meta_xml, title);
        meta_xml.push_str("</name>\n\
             \x20   <author>");
        push_escape_xml(&mut meta_xml, author);
        meta_xml.push_str("</author>\n\
             \x20   <language>");
        push_escape_xml(&mut meta_xml, language);
        meta_xml.push_str("</language>\n\
             \x20   <publisher>");
        push_escape_xml(&mut meta_xml, publisher);
        meta_xml.push_str("</publisher>\n\
             \x20 </head>\n</book-snbf>");
        plain_files.push(("snbf/book.snbf".into(), meta_xml.into_bytes()));

        // 2. Build TOC and chapter SNBC files.
        let chapters = book.chapter_views();
        let mut toc_xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<toc-snbf>\n");

        for (i, chapter) in chapters.iter().enumerate() {
            let ch_filename = format!("chapter_{}.snbc", i);
            let default_title = format!("Chapter {}", i + 1);
            let ch_title = chapter.title.unwrap_or(default_title.as_str());

            toc_xml.push_str("  <chapter src=\"");
            push_escape_xml(&mut toc_xml, &ch_filename);
            toc_xml.push_str("\">");
            push_escape_xml(&mut toc_xml, ch_title);
            toc_xml.push_str("</chapter>\n");

            // Build SNBC chapter XML.
            let stripped = strip_tags_and_unescape(chapter.content);
            let mut snbc = String::with_capacity(80 + ch_title.len() + stripped.len());
            snbc.push_str("<snbc><head><title>");
            push_escape_xml(&mut snbc, ch_title);
            snbc.push_str("</title></head><body><text>");
            push_escape_xml(&mut snbc, &stripped);
            snbc.push_str("</text></body></snbc>");
            plain_files.push((format!("snbc/{}", ch_filename), snbc.into_bytes()));
        }

        toc_xml.push_str("</toc-snbf>");
        plain_files.push(("snbf/toc.snbf".into(), toc_xml.into_bytes()));

        // 3. Collect image resources as binary files.
        for item in book.resources() {
            if item.media_type.starts_with("image/") {
                let name = item.href.rsplit('/').next().unwrap_or(item.href);
                binary_files.push((format!("snbi/{}", name), item.data.to_vec()));
            }
        }

        // Combine all files with attributes (move instead of clone).
        let mut all_files: Vec<(String, Vec<u8>, u32)> = plain_files
            .into_iter()
            .map(|(name, data)| (name, data, ATTR_PLAIN))
            .collect();
        all_files.extend(
            binary_files
                .into_iter()
                .map(|(name, data)| (name, data, ATTR_BINARY)),
        );

        // Sort: plain files first, then binary.
        all_files.sort_by(|a, b| {
            let a_plain = a.2 == ATTR_PLAIN;
            let b_plain = b.2 == ATTR_PLAIN;
            b_plain.cmp(&a_plain).then(a.0.cmp(&b.0))
        });

        let file_count = all_files.len();

        // Build VFAT.
        let mut vfat_entries = Vec::new();
        let mut name_table = Vec::new();
        for (name, data, attr) in &all_files {
            let name_offset = name_table.len() as u32;
            vfat_entries.extend_from_slice(&attr.to_be_bytes());
            vfat_entries.extend_from_slice(&name_offset.to_be_bytes());
            vfat_entries.extend_from_slice(&(data.len() as u32).to_be_bytes());
            name_table.extend_from_slice(name.as_bytes());
            name_table.push(0);
        }
        vfat_entries.extend_from_slice(&name_table);
        let vfat_uncompressed_size = vfat_entries.len();
        let vfat_compressed = snb_zlib_compress(&vfat_entries)?;

        // Build binary stream (raw concatenation).
        let mut bin_stream = Vec::new();
        let mut bin_file_offsets: Vec<usize> = Vec::new();
        for (_, data, attr) in &all_files {
            if *attr == ATTR_BINARY {
                bin_file_offsets.push(bin_stream.len());
                bin_stream.extend_from_slice(data);
            }
        }

        // Build plain stream (bz2-compressed in blocks).
        // Concatenate all plain file data, tracking file start offsets.
        let mut plain_concat = Vec::new();
        let mut plain_file_offsets: Vec<usize> = Vec::new();
        for (_, data, attr) in &all_files {
            if *attr == ATTR_PLAIN {
                plain_file_offsets.push(plain_concat.len());
                plain_concat.extend_from_slice(data);
            }
        }

        // Compress plain data in BLOCK_SIZE (32KB) blocks.
        let mut plain_blocks: Vec<Vec<u8>> = Vec::new();
        let mut block_offset = 0;
        while block_offset < plain_concat.len() {
            let end = (block_offset + BLOCK_SIZE).min(plain_concat.len());
            let block_data = &plain_concat[block_offset..end];
            let compressed = snb_bz2_compress(block_data)?;
            plain_blocks.push(compressed);
            block_offset = end;
        }

        // Build block offset table for the tail.
        // Binary blocks: one "block" at offset 0 for the entire binary stream.
        let bin_block_count = if bin_stream.is_empty() { 0 } else { 1 };

        let mut block_offsets: Vec<i32> = Vec::new();
        if bin_block_count > 0 {
            block_offsets.push(0); // binary stream starts at offset 0
        }
        let mut plain_pos = 0i32;
        for block in &plain_blocks {
            block_offsets.push(plain_pos);
            plain_pos += snb_i32(block.len())?;
        }

        // Build file records for tail.
        let mut file_records: Vec<(i32, i32)> = Vec::new();
        let mut bin_idx = 0;
        let mut plain_idx = 0;
        for (_, _, attr) in &all_files {
            if *attr == ATTR_BINARY {
                let offset = if bin_idx < bin_file_offsets.len() {
                    bin_file_offsets[bin_idx]
                } else {
                    0
                };
                file_records.push((0, snb_i32(offset)?)); // block_index=0 for binary
                bin_idx += 1;
            } else {
                let offset = if plain_idx < plain_file_offsets.len() {
                    plain_file_offsets[plain_idx]
                } else {
                    0
                };
                // Find which block this file starts in.
                let file_start = offset;
                let block_idx = file_start / BLOCK_SIZE;
                let content_offset = file_start % BLOCK_SIZE;
                file_records.push((snb_i32(block_idx)?, snb_i32(content_offset)?));
                plain_idx += 1;
            }
        }

        // Build tail data.
        let mut tail_data = Vec::new();
        for &off in &block_offsets {
            tail_data.extend_from_slice(&off.to_be_bytes());
        }
        for &(block_idx, content_offset) in &file_records {
            tail_data.extend_from_slice(&block_idx.to_be_bytes());
            tail_data.extend_from_slice(&content_offset.to_be_bytes());
        }
        let tail_compressed = snb_zlib_compress(&tail_data)?;

        // Calculate file layout.
        let vfat_start = HEADER_SIZE;
        let bin_start = vfat_start + vfat_compressed.len();
        let plain_start = bin_start + bin_stream.len();
        let plain_stream_size: usize = plain_blocks.iter().map(|b| b.len()).sum();
        let tail_offset = plain_start + plain_stream_size;
        let footer_start = tail_offset + tail_compressed.len();
        let total_size = footer_start + 16;

        // Build the file.
        const MAX_OUTPUT_SIZE: usize = 512 * 1024 * 1024; // 512 MB
        if total_size > MAX_OUTPUT_SIZE {
            return Err(EruditioError::Format(
                "SNB output exceeds maximum allowed size".into(),
            ));
        }
        let mut file = vec![0u8; total_size];

        // Header.
        file[0..8].copy_from_slice(SNB_MAGIC);
        file[0x08..0x0C].copy_from_slice(&0x00008000i32.to_be_bytes());
        file[0x0C..0x10].copy_from_slice(&0x00A3A3A3i32.to_be_bytes());
        file[0x14..0x18].copy_from_slice(&snb_i32(file_count)?.to_be_bytes());
        file[0x18..0x1C].copy_from_slice(&snb_i32(vfat_uncompressed_size)?.to_be_bytes());
        file[0x1C..0x20].copy_from_slice(&snb_i32(vfat_compressed.len())?.to_be_bytes());
        file[0x20..0x24].copy_from_slice(&snb_i32(bin_stream.len())?.to_be_bytes());
        file[0x24..0x28].copy_from_slice(&snb_i32(plain_concat.len())?.to_be_bytes());

        // VFAT.
        file[vfat_start..vfat_start + vfat_compressed.len()].copy_from_slice(&vfat_compressed);

        // Binary stream.
        if !bin_stream.is_empty() {
            file[bin_start..bin_start + bin_stream.len()].copy_from_slice(&bin_stream);
        }

        // Plain stream blocks.
        let mut pos = plain_start;
        for block in &plain_blocks {
            file[pos..pos + block.len()].copy_from_slice(block);
            pos += block.len();
        }

        // Tail.
        file[tail_offset..tail_offset + tail_compressed.len()].copy_from_slice(&tail_compressed);

        // Footer.
        file[footer_start..footer_start + 4]
            .copy_from_slice(&snb_i32(tail_compressed.len())?.to_be_bytes());
        file[footer_start + 4..footer_start + 8]
            .copy_from_slice(&snb_i32(tail_offset)?.to_be_bytes());
        file[footer_start + 8..footer_start + 16].copy_from_slice(SNB_MAGIC);

        output.write_all(&file)?;
        Ok(())
    }
}

/// Converts a `usize` to `i32`, returning an error if the value overflows.
fn snb_i32(value: usize) -> Result<i32> {
    i32::try_from(value).map_err(|_| EruditioError::Format("SNB value too large for i32".into()))
}

/// Compresses data with zlib.
fn snb_zlib_compress(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder
        .write_all(data)
        .map_err(|e| EruditioError::Compression(format!("SNB zlib compression error: {}", e)))?;
    encoder.finish().map_err(|e| {
        EruditioError::Compression(format!("SNB zlib compression finish error: {}", e))
    })
}

/// Compresses data with bz2.
fn snb_bz2_compress(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = BzEncoder::new(Vec::new(), BzCompression::default());
    encoder
        .write_all(data)
        .map_err(|e| EruditioError::Compression(format!("SNB bz2 compression error: {}", e)))?;
    encoder
        .finish()
        .map_err(|e| EruditioError::Compression(format!("SNB bz2 compression finish error: {}", e)))
}

// -- Helpers --

/// Maximum decompression output to prevent decompression bombs.
const MAX_DECOMPRESS_OUTPUT: u64 = 256 * 1024 * 1024; // 256 MB

fn zlib_decompress(data: &[u8], _expected_size: usize) -> Result<Vec<u8>> {
    let decoder = ZlibDecoder::new(data);
    let mut limited = decoder.take(MAX_DECOMPRESS_OUTPUT);
    let mut output = Vec::new();
    limited
        .read_to_end(&mut output)
        .map_err(|e| EruditioError::Compression(format!("SNB zlib decompression error: {}", e)))?;
    Ok(output)
}

fn bz2_decompress(data: &[u8]) -> Result<Vec<u8>> {
    let decoder = BzDecoder::new(data);
    let mut limited = decoder.take(MAX_DECOMPRESS_OUTPUT);
    let mut output = Vec::new();
    limited
        .read_to_end(&mut output)
        .map_err(|e| EruditioError::Compression(format!("SNB bz2 decompression error: {}", e)))?;
    Ok(output)
}

fn read_i32_be(data: &[u8], offset: usize) -> i32 {
    i32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_u32_be(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_cstring(data: &[u8], offset: usize) -> String {
    if offset >= data.len() {
        return String::new(); // out-of-bounds offset from crafted file
    }
    let end = data[offset..]
        .iter()
        .position(|&b| b == 0)
        .map(|p| offset + p)
        .unwrap_or(data.len());
    crate::formats::common::text_utils::bytes_to_string(&data[offset..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use bzip2::Compression as BzCompression;
    use bzip2::write::BzEncoder;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::{Cursor, Write};

    fn zlib_compress(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    fn bz2_compress(data: &[u8]) -> Vec<u8> {
        let mut encoder = BzEncoder::new(Vec::new(), BzCompression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    /// Builds a minimal synthetic SNB file for testing.
    fn build_snb_file(
        title: &str,
        author: &str,
        chapters: &[(&str, &str)], // (chapter_title, chapter_text)
    ) -> Vec<u8> {
        // Build content files.
        let meta_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<book-snbf version="1.0">
  <head>
    <name>{}</name>
    <author>{}</author>
    <language>en</language>
  </head>
</book-snbf>"#,
            title, author
        );

        let mut toc_xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<toc-snbf>\n");
        for (i, (ch_title, _)) in chapters.iter().enumerate() {
            toc_xml.push_str(&format!(
                "  <chapter src=\"ch{}.snbc\">{}</chapter>\n",
                i, ch_title
            ));
        }
        toc_xml.push_str("</toc-snbf>");

        let mut chapter_files: Vec<(String, Vec<u8>)> = Vec::new();
        for (i, (ch_title, ch_text)) in chapters.iter().enumerate() {
            let snbc = format!(
                "<snbc><head><title>{}</title></head><body><text>{}</text></body></snbc>",
                ch_title, ch_text
            );
            chapter_files.push((format!("ch{}.snbc", i), snbc.into_bytes()));
        }

        // All files: meta, toc, chapters — all are "plain" (bz2-compressed).
        let mut all_files: Vec<(String, Vec<u8>, u32)> = vec![
            ("snbf/book.snbf".into(), meta_xml.into_bytes(), ATTR_PLAIN),
            ("snbf/toc.snbf".into(), toc_xml.into_bytes(), ATTR_PLAIN),
        ];
        for (name, data) in &chapter_files {
            all_files.push((format!("snbc/{}", name), data.clone(), ATTR_PLAIN));
        }

        let file_count = all_files.len();

        // Build VFAT.
        let mut vfat = Vec::new();
        let mut name_table = Vec::new();
        for (name, data, attr) in &all_files {
            let name_offset = name_table.len() as u32;
            vfat.extend_from_slice(&attr.to_be_bytes());
            vfat.extend_from_slice(&name_offset.to_be_bytes());
            vfat.extend_from_slice(&(data.len() as u32).to_be_bytes());
            name_table.extend_from_slice(name.as_bytes());
            name_table.push(0); // null terminator
        }
        vfat.extend_from_slice(&name_table);
        let vfat_uncompressed_size = vfat.len();
        let vfat_compressed = zlib_compress(&vfat);

        // Build plain stream: bz2-compress each file into one block.
        // For simplicity, put all files into a single bz2 block.
        let mut plain_uncompressed = Vec::new();
        let mut file_offsets: Vec<usize> = Vec::new();
        for (_, data, _) in &all_files {
            file_offsets.push(plain_uncompressed.len());
            plain_uncompressed.extend_from_slice(data);
        }
        let plain_compressed = bz2_compress(&plain_uncompressed);

        // Layout:
        // [header 0x2C] [vfat compressed] [plain compressed] [tail compressed] [tail footer 16]
        let bin_stream_size: i32 = 0; // no binary files
        let header_size = HEADER_SIZE;
        let plain_start_offset = header_size + vfat_compressed.len();

        // Build tail block.
        // Block offsets: 1 plain block at offset 0.
        // File records: each file at (block_index=0, content_offset).
        let mut tail_data = Vec::new();
        // One block offset: 0
        tail_data.extend_from_slice(&0i32.to_be_bytes());
        // File records.
        for offset in &file_offsets {
            tail_data.extend_from_slice(&0i32.to_be_bytes()); // block_index = 0
            tail_data.extend_from_slice(&(*offset as i32).to_be_bytes());
        }
        let tail_compressed = zlib_compress(&tail_data);
        let tail_offset = plain_start_offset + plain_compressed.len();

        // Total file size.
        let total_size = tail_offset + tail_compressed.len() + 16;
        let mut file = vec![0u8; total_size];

        // Write header.
        file[0..8].copy_from_slice(SNB_MAGIC);
        file[0x08..0x0C].copy_from_slice(&0x00008000i32.to_be_bytes());
        file[0x0C..0x10].copy_from_slice(&0x00A3A3A3i32.to_be_bytes());
        file[0x14..0x18].copy_from_slice(&(file_count as i32).to_be_bytes());
        file[0x18..0x1C].copy_from_slice(&(vfat_uncompressed_size as i32).to_be_bytes());
        file[0x1C..0x20].copy_from_slice(&(vfat_compressed.len() as i32).to_be_bytes());
        file[0x20..0x24].copy_from_slice(&bin_stream_size.to_be_bytes());
        file[0x24..0x28].copy_from_slice(&(plain_uncompressed.len() as i32).to_be_bytes());

        // Write VFAT.
        file[header_size..header_size + vfat_compressed.len()].copy_from_slice(&vfat_compressed);

        // Write plain stream.
        file[plain_start_offset..plain_start_offset + plain_compressed.len()]
            .copy_from_slice(&plain_compressed);

        // Write tail.
        file[tail_offset..tail_offset + tail_compressed.len()].copy_from_slice(&tail_compressed);

        // Write tail footer.
        let footer_start = tail_offset + tail_compressed.len();
        file[footer_start..footer_start + 4]
            .copy_from_slice(&(tail_compressed.len() as i32).to_be_bytes());
        file[footer_start + 4..footer_start + 8]
            .copy_from_slice(&(tail_offset as i32).to_be_bytes());
        file[footer_start + 8..footer_start + 16].copy_from_slice(SNB_MAGIC);

        file
    }

    #[test]
    fn reads_snb_with_chapters() {
        let data = build_snb_file(
            "Test SNB Book",
            "SNB Author",
            &[
                ("Chapter One", "Hello from SNB chapter one!"),
                ("Chapter Two", "Content of chapter two."),
            ],
        );

        let mut cursor = Cursor::new(data);
        let book = SnbReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Test SNB Book"));
        assert_eq!(book.metadata.authors, vec!["SNB Author"]);
        assert_eq!(book.chapters().len(), 2);
        let content: String = book.chapter_views().iter().map(|c| c.content).collect();
        assert!(content.contains("Hello from SNB chapter one!"));
        assert!(content.contains("Content of chapter two."));
    }

    #[test]
    fn reads_metadata_from_snb() {
        let data = build_snb_file("My Book", "Author Name", &[("Ch1", "Text")]);
        let mut cursor = Cursor::new(data);
        let book = SnbReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("My Book"));
        assert_eq!(book.metadata.authors, vec!["Author Name"]);
        assert!(book.metadata.language.as_deref() == Some("en"));
    }

    #[test]
    fn rejects_invalid_magic() {
        let mut data = vec![0u8; HEADER_SIZE + 32];
        data[0..8].copy_from_slice(b"INVALID!");

        let mut cursor = Cursor::new(data);
        let result = SnbReader::new().read_book(&mut cursor);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("magic"));
    }

    #[test]
    fn rejects_short_file() {
        let data = vec![0u8; 10];
        let mut cursor = Cursor::new(data);
        let result = SnbReader::new().read_book(&mut cursor);
        assert!(result.is_err());
    }

    #[test]
    fn snbc_to_html_converts_text_and_img() {
        let xml = "<snbc><body><text>Hello world</text><img>pic.jpg</img></body></snbc>";
        let html = snbc_to_html(xml);
        assert!(html.contains("<p>Hello world</p>"));
        assert!(html.contains("<img src=\"pic.jpg\""));
    }

    #[test]
    fn parse_toc_extracts_chapters() {
        let xml = r#"<toc-snbf>
            <chapter src="ch0.snbc">First Chapter</chapter>
            <chapter src="ch1.snbc">Second Chapter</chapter>
        </toc-snbf>"#;
        let chapters = parse_toc(xml);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].title, "First Chapter");
        assert_eq!(chapters[0].src, "ch0.snbc");
        assert_eq!(chapters[1].title, "Second Chapter");
    }

    #[test]
    fn snb_writer_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("SNB Write Test".into());
        book.metadata.authors = vec!["SNB Author".into()];
        book.metadata.language = Some("en".into());
        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello Bambook!</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        SnbWriter::new().write_book(&book, &mut output).unwrap();

        // Read it back.
        let mut cursor = Cursor::new(output);
        let decoded = SnbReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("SNB Write Test"));
        assert_eq!(decoded.metadata.authors, vec!["SNB Author"]);
        let content: String = decoded
            .chapters()
            .iter()
            .map(|c| c.content.clone())
            .collect();
        assert!(content.contains("Hello Bambook!"));
    }
}
