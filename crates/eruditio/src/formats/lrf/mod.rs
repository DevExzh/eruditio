//! LRF (Sony BBeB / Broad Band eBook) reader.
//!
//! Reads Sony Reader LRF files by parsing the binary header, object index,
//! and tag-based object system. Extracts metadata from the compressed XML
//! block and text content from Text objects following the PageTree traversal
//! order.

pub mod header;
pub mod objects;
pub mod tags;
pub mod text;
pub mod writer;

use crate::domain::{Book, Chapter, FormatReader};
use crate::error::Result;
use std::io::Read;

pub use writer::LrfWriter;

use ahash::AHashMap as HashMap;
use header::{LrfHeader, parse_metadata};
use objects::{LrfObject, ObjType, parse_objects, parse_toc_stream};
use tags::TAG_REFSTREAM;
use text::{tokenize_text_stream, tokens_to_html};

/// An image reference extracted from an LRF Image/ImageStream pair.
struct ImageRef {
    id: String,
    href: String,
    data: Vec<u8>,
    media_type: &'static str,
}

/// LRF ebook format reader.
#[derive(Default)]
pub struct LrfReader;

impl LrfReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for LrfReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;

        let header = LrfHeader::parse(&data)?;
        let meta = parse_metadata(&data, &header)?;

        let objects = parse_objects(
            &data,
            header.object_index_offset,
            header.number_of_objects,
            header.xor_key,
        )?;

        let mut book = Book::new();

        // Populate metadata.
        book.metadata.title = meta.title;
        if let Some(author) = meta.author {
            book.metadata.authors = vec![author];
        }
        book.metadata.publisher = meta.publisher;
        if let Some(lang) = meta.language {
            book.metadata.language = Some(lang);
        }
        if let Some(desc) = meta.free_text {
            book.metadata.description = Some(desc);
        }

        // Parse TOC for chapter labels.
        let toc_labels = build_toc_labels(&objects);

        // Traverse reading order: PageTree → Page → Block → Text/Image.
        let (chapters, all_images) = extract_chapters(&objects, &toc_labels);

        if chapters.is_empty() {
            // Fallback: collect all Text objects as a single chapter.
            let html = collect_all_text(&objects);
            book.add_chapter(Chapter {
                title: book.metadata.title.clone(),
                content: if html.is_empty() {
                    "<p></p>".to_string()
                } else {
                    html
                },
                id: Some("lrf_main".into()),
            });
        } else {
            for (idx, (title, content)) in chapters.into_iter().enumerate() {
                book.add_chapter(Chapter {
                    title,
                    content,
                    id: Some(format!("lrf_ch_{}", idx)),
                });
            }
        }

        // Add extracted images as book resources (consume to move data, avoiding clone).
        for img in all_images {
            book.add_resource(&img.id, &img.href, img.data, img.media_type);
        }

        Ok(book)
    }
}

/// Builds a map from page object ID → chapter label from TOC entries.
fn build_toc_labels(objects: &HashMap<u32, LrfObject>) -> HashMap<u32, String> {
    let mut labels = HashMap::new();

    for obj in objects.values() {
        if obj.obj_type == ObjType::TOCObject
            && let Some(stream) = &obj.stream_data
            && let Ok(entries) = parse_toc_stream(stream)
        {
            for entry in entries {
                labels.insert(entry.refpage, entry.label);
            }
        }
    }

    labels
}

/// Extracts chapters by following the PageTree → Page → Block → Text/Image chain.
fn extract_chapters(
    objects: &HashMap<u32, LrfObject>,
    toc_labels: &HashMap<u32, String>,
) -> (Vec<(Option<String>, String)>, Vec<ImageRef>) {
    let mut chapters: Vec<(Option<String>, String)> = Vec::new();
    let mut all_images: Vec<ImageRef> = Vec::new();

    // Find all PageTree objects (reading order roots).
    let mut page_trees: Vec<&LrfObject> = objects
        .values()
        .filter(|o| o.obj_type == ObjType::PageTree)
        .collect();
    page_trees.sort_by_key(|o| o.id);

    for page_tree in &page_trees {
        let page_ids = page_tree.contained_object_ids();

        for &page_id in &page_ids {
            let page = match objects.get(&page_id) {
                Some(p) if p.obj_type == ObjType::Page => p,
                _ => continue,
            };

            let label = toc_labels.get(&page_id).cloned();
            let (html, images) = extract_page_html(page, objects);

            all_images.extend(images);

            if html.trim().is_empty() {
                continue;
            }

            // If there's a TOC label for this page, start a new chapter.
            if label.is_some() || chapters.is_empty() {
                chapters.push((label, html));
            } else if let Some(last) = chapters.last_mut() {
                // Append to current chapter.
                last.1.push_str(&html);
            }
        }
    }

    (chapters, all_images)
}

/// Extracts HTML content and image references from a single Page object.
fn extract_page_html(
    page: &LrfObject,
    objects: &HashMap<u32, LrfObject>,
) -> (String, Vec<ImageRef>) {
    let mut html = String::new();
    let mut images: Vec<ImageRef> = Vec::new();

    // Page stream contains block references.
    // Also check contained objects list from tags.
    let block_ids = page.contained_object_ids();

    // If no contained objects, try to find blocks from the stream.
    let ids_to_check = if block_ids.is_empty() {
        extract_ids_from_stream(page)
    } else {
        block_ids
    };

    for &block_id in &ids_to_check {
        let block = match objects.get(&block_id) {
            Some(b) => b,
            None => continue,
        };

        match block.obj_type {
            ObjType::Block => {
                if let Some(linked_id) = block.link_id()
                    && let Some(linked_obj) = objects.get(&linked_id)
                {
                    match linked_obj.obj_type {
                        ObjType::Text | ObjType::SimpleText => {
                            html.push_str(&text_object_to_html(linked_obj));
                        },
                        ObjType::Image => {
                            if let Some(img_ref) = extract_image(linked_obj, objects) {
                                html.push_str("<img src=\"");
                                html.push_str(&img_ref.href);
                                html.push_str("\" />");
                                images.push(img_ref);
                            }
                        },
                        _ => {},
                    }
                }
            },
            ObjType::Canvas | ObjType::Header | ObjType::Footer => {
                // Canvas may contain blocks.
                let canvas_blocks = block.contained_object_ids();
                for &cb_id in &canvas_blocks {
                    if let Some(cb) = objects.get(&cb_id)
                        && cb.obj_type == ObjType::Block
                        && let Some(linked_id) = cb.link_id()
                        && let Some(linked_obj) = objects.get(&linked_id)
                    {
                        match linked_obj.obj_type {
                            ObjType::Text | ObjType::SimpleText => {
                                html.push_str(&text_object_to_html(linked_obj));
                            },
                            ObjType::Image => {
                                if let Some(img_ref) = extract_image(linked_obj, objects) {
                                    html.push_str(&format!("<img src=\"{}\" />", img_ref.href));
                                    images.push(img_ref);
                                }
                            },
                            _ => {},
                        }
                    }
                }
            },
            ObjType::Text | ObjType::SimpleText => {
                // Direct text reference.
                html.push_str(&text_object_to_html(block));
            },
            _ => {},
        }
    }

    (html, images)
}

/// Converts a Text object's stream data to HTML.
fn text_object_to_html(obj: &LrfObject) -> String {
    let Some(stream) = &obj.stream_data else {
        return String::new();
    };

    let tokens = tokenize_text_stream(stream);
    tokens_to_html(&tokens)
}

/// Extracts image data from an Image object by following its RefStream tag
/// to the corresponding ImageStream object.
fn extract_image(image_obj: &LrfObject, objects: &HashMap<u32, LrfObject>) -> Option<ImageRef> {
    // Find the refstream tag pointing to the ImageStream object.
    let stream_id = image_obj
        .tags
        .iter()
        .find(|t| t.id == TAG_REFSTREAM)
        .map(|t| t.as_u32())?;

    let stream_obj = objects.get(&stream_id)?;
    if stream_obj.obj_type != ObjType::ImageStream {
        return None;
    }

    let data = stream_obj.stream_data.as_ref()?.clone();
    if data.is_empty() {
        return None;
    }

    let (media_type, ext) = match stream_obj.stream_flags & 0xFF {
        0x11 => ("image/jpeg", "jpg"),
        0x12 => ("image/png", "png"),
        0x13 => ("image/bmp", "bmp"),
        0x14 => ("image/gif", "gif"),
        _ => ("image/jpeg", "jpg"), // default to JPEG
    };

    let id = format!("lrf_img_{}", image_obj.id);
    let href = format!("images/{}.{}", id, ext);

    Some(ImageRef {
        id,
        href,
        data,
        media_type,
    })
}

/// Extracts object IDs referenced from a Page/Canvas stream.
fn extract_ids_from_stream(obj: &LrfObject) -> Vec<u32> {
    let Some(stream) = &obj.stream_data else {
        return Vec::new();
    };

    // The stream for Page/Canvas objects contains tags;
    // look for Link tags (0xF503) which reference child objects.
    let mut ids = Vec::new();
    let mut pos = 0;
    while pos + 5 < stream.len() {
        if pos + 1 < stream.len() && stream[pos + 1] == 0xF5 && stream[pos] == 0x03 {
            // Link tag: 4-byte u32 payload.
            if pos + 6 <= stream.len() {
                let id = u32::from_le_bytes([
                    stream[pos + 2],
                    stream[pos + 3],
                    stream[pos + 4],
                    stream[pos + 5],
                ]);
                ids.push(id);
                pos += 6;
                continue;
            }
        }
        pos += 1;
    }

    ids
}

/// Fallback: collects all Text objects into a single HTML string.
fn collect_all_text(objects: &HashMap<u32, LrfObject>) -> String {
    let mut text_objs: Vec<&LrfObject> = objects
        .values()
        .filter(|o| o.obj_type == ObjType::Text || o.obj_type == ObjType::SimpleText)
        .collect();
    text_objs.sort_by_key(|o| o.id);

    let mut html = String::new();
    for obj in text_objs {
        html.push_str(&text_object_to_html(obj));
    }
    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::ZlibEncoder;
    use std::io::{Cursor, Write as IoWrite};

    /// Builds a minimal synthetic LRF file for testing.
    /// Contains: header + compressed metadata XML + one Text object.
    fn build_minimal_lrf(title: &str, author: &str, text_content: &str) -> Vec<u8> {
        // 1. Build compressed metadata XML.
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Info version="1.1">
  <BookInfo>
    <Title>{}</Title>
    <Author>{}</Author>
  </BookInfo>
  <DocInfo>
    <Language>en</Language>
  </DocInfo>
</Info>"#,
            title, author
        );
        let compressed_xml = zlib_compress(xml.as_bytes());
        let compressed_info_size = (compressed_xml.len() + 4) as u16;

        // 2. Build a Text object with inline content.
        let text_utf16: Vec<u8> = text_content
            .encode_utf16()
            .flat_map(|c| c.to_le_bytes())
            .collect();

        // Text stream: P_START + raw_text + P_END
        let mut text_stream = Vec::new();
        // P start tag: 0xA1 0xF5 + 6 bytes payload
        text_stream.extend_from_slice(&[0xA1, 0xF5, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        // Raw UTF-16LE text (not tagged — direct inline)
        text_stream.extend_from_slice(&text_utf16);
        // P end tag: 0xA2 0xF5
        text_stream.extend_from_slice(&[0xA2, 0xF5]);

        // 3. Build object bytes for a Text object (obj_id=10, obj_type=0x0A).
        let text_obj_bytes = build_object_bytes(10, 0x0A, Some(&text_stream), false);

        // 4. Build object bytes for a Block (obj_id=5, obj_type=0x06) linking to Text obj 10.
        let mut block_stream = Vec::new();
        // Link tag: 0x03 0xF5 + u32 target_id
        block_stream.extend_from_slice(&[0x03, 0xF5]);
        block_stream.extend_from_slice(&10u32.to_le_bytes());
        let block_obj_bytes = build_object_bytes(5, 0x06, Some(&block_stream), false);

        // 5. Build Page object (obj_id=2, obj_type=0x02) with ContainedObjectsList [5].
        let page_obj_bytes = build_object_with_contained(2, 0x02, &[5]);

        // 6. Build PageTree object (obj_id=1, obj_type=0x01) with ContainedObjectsList [2].
        let page_tree_bytes = build_object_with_contained(1, 0x01, &[2]);

        // 7. Calculate layout.
        let info_start = 0x58usize;
        let objects_start = info_start + compressed_xml.len();
        let obj_data = [
            &page_tree_bytes[..],
            &page_obj_bytes[..],
            &block_obj_bytes[..],
            &text_obj_bytes[..],
        ];
        let obj_ids: [u32; 4] = [1, 2, 5, 10];

        let mut obj_offsets = Vec::new();
        let mut pos = objects_start;
        for od in &obj_data {
            obj_offsets.push(pos);
            pos += od.len();
        }
        let obj_index_offset = pos;

        // 8. Build the file.
        let total_size = obj_index_offset + 4 * 16; // 4 objects × 16 bytes each
        let mut file = vec![0u8; total_size];

        // Header.
        file[0..6].copy_from_slice(&[0x4C, 0x00, 0x52, 0x00, 0x46, 0x00]); // magic
        file[0x08..0x0A].copy_from_slice(&1000u16.to_le_bytes()); // version
        file[0x0A..0x0C].copy_from_slice(&0u16.to_le_bytes()); // xor_key (no scramble)
        file[0x0C..0x10].copy_from_slice(&1u32.to_le_bytes()); // root_object_id
        file[0x10..0x18].copy_from_slice(&4u64.to_le_bytes()); // number_of_objects
        file[0x18..0x20].copy_from_slice(&(obj_index_offset as u64).to_le_bytes());
        file[0x24] = 1; // binding LR
        file[0x26..0x28].copy_from_slice(&166u16.to_le_bytes()); // dpi
        file[0x2A..0x2C].copy_from_slice(&600u16.to_le_bytes()); // width
        file[0x2C..0x2E].copy_from_slice(&775u16.to_le_bytes()); // height
        file[0x2E] = 24; // color depth
        file[0x4C..0x4E].copy_from_slice(&compressed_info_size.to_le_bytes());

        // Compressed metadata.
        file[info_start..info_start + compressed_xml.len()].copy_from_slice(&compressed_xml);

        // Object data.
        for (i, od) in obj_data.iter().enumerate() {
            file[obj_offsets[i]..obj_offsets[i] + od.len()].copy_from_slice(od);
        }

        // Object index.
        for i in 0..4 {
            let idx_base = obj_index_offset + i * 16;
            file[idx_base..idx_base + 4].copy_from_slice(&obj_ids[i].to_le_bytes());
            file[idx_base + 4..idx_base + 8]
                .copy_from_slice(&(obj_offsets[i] as u32).to_le_bytes());
            file[idx_base + 8..idx_base + 12]
                .copy_from_slice(&(obj_data[i].len() as u32).to_le_bytes());
        }

        file
    }

    /// Builds raw bytes for an LRF object with optional stream.
    fn build_object_bytes(
        obj_id: u32,
        obj_type: u16,
        stream: Option<&[u8]>,
        _compressed: bool,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();

        // ObjectStart: 0x00 0xF5 + u32 obj_id + u16 obj_type
        bytes.extend_from_slice(&[0x00, 0xF5]);
        bytes.extend_from_slice(&obj_id.to_le_bytes());
        bytes.extend_from_slice(&obj_type.to_le_bytes());

        if let Some(stream_data) = stream {
            // StreamFlags: 0x54 0xF5 + u16 flags (no compression, no scramble)
            bytes.extend_from_slice(&[0x54, 0xF5, 0x00, 0x00]);

            // StreamSize: 0x04 0xF5 + u32 size
            bytes.extend_from_slice(&[0x04, 0xF5]);
            bytes.extend_from_slice(&(stream_data.len() as u32).to_le_bytes());

            // StreamStart: 0x05 0xF5
            bytes.extend_from_slice(&[0x05, 0xF5]);

            // Stream data.
            bytes.extend_from_slice(stream_data);

            // StreamEnd: 0x06 0xF5
            bytes.extend_from_slice(&[0x06, 0xF5]);
        }

        // ObjectEnd: 0x01 0xF5
        bytes.extend_from_slice(&[0x01, 0xF5]);

        bytes
    }

    /// Builds an object with a ContainedObjectsList tag.
    fn build_object_with_contained(obj_id: u32, obj_type: u16, ids: &[u32]) -> Vec<u8> {
        let mut bytes = Vec::new();

        // ObjectStart
        bytes.extend_from_slice(&[0x00, 0xF5]);
        bytes.extend_from_slice(&obj_id.to_le_bytes());
        bytes.extend_from_slice(&obj_type.to_le_bytes());

        // ContainedObjectsList: 0x0B 0xF5 + u16 count + count × u32
        bytes.extend_from_slice(&[0x0B, 0xF5]);
        bytes.extend_from_slice(&(ids.len() as u16).to_le_bytes());
        for &id in ids {
            bytes.extend_from_slice(&id.to_le_bytes());
        }

        // ObjectEnd
        bytes.extend_from_slice(&[0x01, 0xF5]);

        bytes
    }

    fn zlib_compress(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn reads_minimal_lrf() {
        let data = build_minimal_lrf("Test LRF Book", "LRF Author", "Hello from LRF!");
        let mut cursor = Cursor::new(data);
        let book = LrfReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Test LRF Book"));
        assert_eq!(book.metadata.authors, vec!["LRF Author"]);
        let content: String = book.chapter_views().iter().map(|c| c.content).collect();
        assert!(content.contains("Hello from LRF!"));
    }

    #[test]
    fn reads_metadata_only_lrf() {
        let data = build_minimal_lrf("Metadata Book", "Some Author", "Content");
        let mut cursor = Cursor::new(data);
        let book = LrfReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Metadata Book"));
        assert_eq!(book.metadata.authors, vec!["Some Author"]);
        assert!(book.metadata.language.as_deref() == Some("en"));
    }

    #[test]
    fn rejects_invalid_magic() {
        let mut data = vec![0u8; 0x60];
        data[0] = 0xFF;
        let mut cursor = Cursor::new(data);
        let result = LrfReader::new().read_book(&mut cursor);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_short_file() {
        let data = vec![0x4C, 0x00, 0x52, 0x00, 0x46, 0x00]; // just magic
        let mut cursor = Cursor::new(data);
        let result = LrfReader::new().read_book(&mut cursor);
        assert!(result.is_err());
    }
}
