//! LRF file header parsing and metadata extraction.
#![allow(dead_code)]

use crate::error::{EruditioError, Result};
use flate2::bufread::ZlibDecoder;
use std::io::Read as IoRead;

/// LRF magic bytes: "LRF" in UTF-16LE followed by two null bytes.
const LRF_MAGIC: [u8; 6] = [0x4C, 0x00, 0x52, 0x00, 0x46, 0x00];

/// Minimum header size (for version > 800).
const MIN_HEADER_SIZE: usize = 0x58;

/// Parsed LRF file header.
pub(crate) struct LrfHeader {
    pub version: u16,
    pub xor_key: u16,
    pub root_object_id: u32,
    pub number_of_objects: u64,
    pub object_index_offset: u64,
    pub binding: u8,
    pub dpi: u16,
    pub width: u16,
    pub height: u16,
    pub color_depth: u8,
    pub toc_object_id: u32,
    pub toc_object_offset: u32,
    pub compressed_info_size: u16,
    pub thumbnail_type: Option<u16>,
    pub thumbnail_size: Option<u32>,
}

/// Metadata extracted from the compressed XML block in the header.
#[derive(Default)]
pub(crate) struct LrfMetadata {
    pub title: Option<String>,
    pub title_reading: Option<String>,
    pub author: Option<String>,
    pub author_reading: Option<String>,
    pub publisher: Option<String>,
    pub category: Option<String>,
    pub classification: Option<String>,
    pub free_text: Option<String>,
    pub language: Option<String>,
    pub creator: Option<String>,
    pub creation_date: Option<String>,
    pub book_id: Option<String>,
}

impl LrfHeader {
    /// Parses the LRF header from the start of the file data.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < MIN_HEADER_SIZE {
            return Err(EruditioError::Format(
                "LRF file too short for header".into(),
            ));
        }

        if data[0..6] != LRF_MAGIC {
            return Err(EruditioError::Format(
                "Invalid LRF magic (expected UTF-16LE 'LRF')".into(),
            ));
        }

        let version = read_u16_le(data, 0x08);
        let xor_key = read_u16_le(data, 0x0A);
        let root_object_id = read_u32_le(data, 0x0C);
        let number_of_objects = read_u64_le(data, 0x10);
        let object_index_offset = read_u64_le(data, 0x18);
        let binding = data[0x24];
        let dpi = read_u16_le(data, 0x26);
        let width = read_u16_le(data, 0x2A);
        let height = read_u16_le(data, 0x2C);
        let color_depth = data[0x2E];
        let toc_object_id = read_u32_le(data, 0x44);
        let toc_object_offset = read_u32_le(data, 0x48);
        let compressed_info_size = read_u16_le(data, 0x4C);

        let (thumbnail_type, thumbnail_size) = if version > 800 {
            (Some(read_u16_le(data, 0x4E)), Some(read_u32_le(data, 0x50)))
        } else {
            (None, None)
        };

        Ok(Self {
            version,
            xor_key,
            root_object_id,
            number_of_objects,
            object_index_offset,
            binding,
            dpi,
            width,
            height,
            color_depth,
            toc_object_id,
            toc_object_offset,
            compressed_info_size,
            thumbnail_type,
            thumbnail_size,
        })
    }

    /// Returns the byte offset where the compressed metadata XML begins.
    pub fn info_start(&self) -> usize {
        if self.version > 800 { 0x58 } else { 0x53 }
    }

    /// Returns the byte length of the compressed metadata block.
    pub fn compressed_info_len(&self) -> usize {
        if self.compressed_info_size > 4 {
            (self.compressed_info_size - 4) as usize
        } else {
            0
        }
    }
}

/// Extracts metadata from the compressed XML block in the LRF header.
pub(crate) fn parse_metadata(data: &[u8], header: &LrfHeader) -> Result<LrfMetadata> {
    let info_len = header.compressed_info_len();
    if info_len == 0 {
        return Ok(LrfMetadata::default());
    }

    let start = header.info_start();
    let end = start + info_len;
    if end > data.len() {
        return Err(EruditioError::Format(
            "LRF metadata block extends past end of file".into(),
        ));
    }

    let compressed = &data[start..end];
    let xml_bytes = zlib_decompress(compressed)?;

    // Strip UTF-8 BOM if present.
    let xml_str = if xml_bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&xml_bytes[3..])
    } else {
        String::from_utf8_lossy(&xml_bytes)
    };

    parse_metadata_xml(&xml_str)
}

/// Parses the metadata XML into structured fields.
fn parse_metadata_xml(xml: &str) -> Result<LrfMetadata> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(xml);
    let mut meta = LrfMetadata::default();

    let mut current_tag = String::new();
    let mut in_book_info = false;
    let mut in_doc_info = false;
    let mut pending_reading = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match name.as_str() {
                    "BookInfo" => in_book_info = true,
                    "DocInfo" => in_doc_info = true,
                    _ => {
                        current_tag = name.clone();
                        // Check for "reading" attribute on Title and Author.
                        if (name == "Title" || name == "Author") && in_book_info {
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"reading" {
                                    pending_reading =
                                        Some(String::from_utf8_lossy(&attr.value).to_string());
                                }
                            }
                        }
                    },
                }
            },
            Ok(Event::End(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match name.as_str() {
                    "BookInfo" => in_book_info = false,
                    "DocInfo" => in_doc_info = false,
                    _ => {},
                }
                current_tag.clear();
                pending_reading = None;
            },
            Ok(Event::Text(ref e)) => {
                let text = String::from_utf8_lossy(&e.clone().into_inner()).to_string();
                if text.trim().is_empty() {
                    continue;
                }
                if in_book_info {
                    match current_tag.as_str() {
                        "Title" => {
                            meta.title = Some(text);
                            meta.title_reading = pending_reading.take();
                        },
                        "Author" => {
                            meta.author = Some(text);
                            meta.author_reading = pending_reading.take();
                        },
                        "Publisher" => meta.publisher = Some(text),
                        "Category" => meta.category = Some(text),
                        "Classification" => meta.classification = Some(text),
                        "FreeText" => meta.free_text = Some(text),
                        "BookID" => meta.book_id = Some(text),
                        _ => {},
                    }
                } else if in_doc_info {
                    match current_tag.as_str() {
                        "Language" => meta.language = Some(text),
                        "Creator" => meta.creator = Some(text),
                        "CreationDate" => meta.creation_date = Some(text),
                        _ => {},
                    }
                }
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {},
        }
    }

    Ok(meta)
}

/// Standard zlib decompression with output size cap to prevent decompression bombs.
fn zlib_decompress(data: &[u8]) -> Result<Vec<u8>> {
    const MAX_DECOMPRESS: u64 = 256 * 1024 * 1024; // 256 MB
    let decoder = ZlibDecoder::new(data);
    let mut limited = decoder.take(MAX_DECOMPRESS);
    let mut output = Vec::new();
    limited
        .read_to_end(&mut output)
        .map_err(|e| EruditioError::Compression(format!("LRF zlib decompression error: {}", e)))?;
    Ok(output)
}

pub(crate) fn read_u16_le(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

pub(crate) fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

pub(crate) fn read_u64_le(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_lrf_header(
        version: u16,
        xor_key: u16,
        num_objects: u64,
        obj_index_offset: u64,
    ) -> Vec<u8> {
        let mut data = vec![0u8; 0x60];
        data[0..6].copy_from_slice(&LRF_MAGIC);
        data[0x08..0x0A].copy_from_slice(&version.to_le_bytes());
        data[0x0A..0x0C].copy_from_slice(&xor_key.to_le_bytes());
        data[0x0C..0x10].copy_from_slice(&1u32.to_le_bytes()); // root_object_id
        data[0x10..0x18].copy_from_slice(&num_objects.to_le_bytes());
        data[0x18..0x20].copy_from_slice(&obj_index_offset.to_le_bytes());
        data[0x24] = 1; // binding LR
        data[0x26..0x28].copy_from_slice(&166u16.to_le_bytes()); // dpi
        data[0x2A..0x2C].copy_from_slice(&600u16.to_le_bytes()); // width
        data[0x2C..0x2E].copy_from_slice(&775u16.to_le_bytes()); // height
        data[0x2E] = 24; // color_depth
        data
    }

    #[test]
    fn parses_valid_header() {
        let data = build_lrf_header(1000, 0x1234, 10, 0x100);
        let header = LrfHeader::parse(&data).unwrap();
        assert_eq!(header.version, 1000);
        assert_eq!(header.xor_key, 0x1234);
        assert_eq!(header.number_of_objects, 10);
        assert_eq!(header.object_index_offset, 0x100);
        assert_eq!(header.dpi, 166);
        assert_eq!(header.width, 600);
        assert_eq!(header.height, 775);
        assert!(header.thumbnail_type.is_some());
    }

    #[test]
    fn rejects_invalid_magic() {
        let mut data = build_lrf_header(1000, 0, 0, 0);
        data[0] = 0xFF;
        let result = LrfHeader::parse(&data);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_short_data() {
        let data = vec![0u8; 10];
        let result = LrfHeader::parse(&data);
        assert!(result.is_err());
    }

    #[test]
    fn parses_metadata_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Info version="1.1">
  <BookInfo>
    <Title reading="test sort">Test Book</Title>
    <Author reading="last, first">Test Author</Author>
    <Publisher>Test Publisher</Publisher>
    <BookID>12345</BookID>
  </BookInfo>
  <DocInfo>
    <Language>en</Language>
    <CreationDate>2024-01-01</CreationDate>
  </DocInfo>
</Info>"#;

        let meta = super::parse_metadata_xml(xml).unwrap();
        assert_eq!(meta.title.as_deref(), Some("Test Book"));
        assert_eq!(meta.title_reading.as_deref(), Some("test sort"));
        assert_eq!(meta.author.as_deref(), Some("Test Author"));
        assert_eq!(meta.publisher.as_deref(), Some("Test Publisher"));
        assert_eq!(meta.language.as_deref(), Some("en"));
        assert_eq!(meta.book_id.as_deref(), Some("12345"));
    }
}
