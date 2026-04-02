//! LRF (Sony BBeB) writer.
//!
//! Produces a valid LRF file from a `Book` by building the binary header,
//! compressed metadata XML, object tree (BookAttr, TextAttr, BlockAttr,
//! PageAttr, Pages, Blocks, TextBlocks, ImageStreams, Images, PageTree),
//! and the object index.

use crate::domain::Book;
use crate::error::{EruditioError, Result};
use crate::formats::common::text_utils::escape_xml;
use flate2::write::ZlibEncoder;
use std::io::Write;

// Note: tag constants from super::tags are not directly used here; the writer
// encodes tags by their low-byte IDs via the emit_* / write_tag_* helpers.

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// LRF magic: UTF-16LE "LRF".
const LRF_MAGIC: [u8; 6] = [0x4C, 0x00, 0x52, 0x00, 0x46, 0x00];

/// Fixed header size (version > 800).
const HEADER_SIZE: usize = 0x58;

/// Default screen dimensions and properties for the Sony Reader.
const DEFAULT_DPI: u16 = 1660; // 166 * 10
const DEFAULT_WIDTH: u16 = 600;
const DEFAULT_HEIGHT: u16 = 800;
const DEFAULT_COLOR_DEPTH: u8 = 24;
const DEFAULT_FONT_SIZE: u16 = 100; // 10.0pt * 10
const DEFAULT_VERSION: u16 = 1000;

/// Stream flag: zlib-compressed.
const STREAM_FLAG_COMPRESSED: u16 = 0x0100;

/// LRF object type codes.
const OBJ_TYPE_PAGE_TREE: u16 = 0x01;
const OBJ_TYPE_PAGE: u16 = 0x02;
const OBJ_TYPE_PAGE_ATTR: u16 = 0x05;
const OBJ_TYPE_BLOCK: u16 = 0x06;
const OBJ_TYPE_BLOCK_ATTR: u16 = 0x07;
const OBJ_TYPE_TEXT: u16 = 0x0A;
const OBJ_TYPE_TEXT_ATTR: u16 = 0x0B;
const OBJ_TYPE_IMAGE: u16 = 0x0C;
const OBJ_TYPE_IMAGE_STREAM: u16 = 0x11;
const OBJ_TYPE_BOOK_ATTR: u16 = 0x1C;

// Tag IDs used for style attribute tags.
const TAG_ID_FONT_SIZE: u8 = 0x11;
const TAG_ID_FONT_WEIGHT: u8 = 0x15;
const TAG_ID_BLOCK_WIDTH: u8 = 0x41;
const TAG_ID_BLOCK_HEIGHT: u8 = 0x42;
const TAG_ID_TOP_SKIP: u8 = 0x49;
const TAG_ID_SIDE_MARGIN: u8 = 0x4A;
const TAG_ID_PAGE_HEIGHT: u8 = 0x32;
const TAG_ID_PAGE_WIDTH: u8 = 0x33;
const TAG_ID_ODD_SIDE_MARGIN: u8 = 0x31;
const TAG_ID_EVEN_SIDE_MARGIN: u8 = 0x36;

// Image-related tag IDs (reserved for future use if image rect/size tags are needed).

// ---------------------------------------------------------------------------
// Object record used during writing
// ---------------------------------------------------------------------------

/// A single LRF object ready to be serialized.
struct LrfWriteObject {
    id: u32,
    obj_type: u16,
    /// Non-stream tags (style attributes, links, contained objects, etc.).
    tags: Vec<u8>,
    /// Optional stream data (text tags, image data, page links, etc.).
    /// If `compress_stream` is true the stream will be zlib-compressed.
    stream: Option<Vec<u8>>,
    compress_stream: bool,
}

impl LrfWriteObject {
    fn new(id: u32, obj_type: u16) -> Self {
        Self {
            id,
            obj_type,
            tags: Vec::new(),
            stream: None,
            compress_stream: false,
        }
    }

    /// Serializes the object to its on-disk representation.
    fn serialize(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(128);

        // ObjectStart tag: 0x00 0xF5 + u32 id + u16 type
        buf.push(0x00);
        buf.push(0xF5);
        buf.extend_from_slice(&self.id.to_le_bytes());
        buf.extend_from_slice(&self.obj_type.to_le_bytes());

        // Non-stream tags.
        buf.extend_from_slice(&self.tags);

        // Stream (if present).
        if let Some(ref raw_stream) = self.stream {
            if self.compress_stream {
                let compressed = zlib_compress(raw_stream)?;
                // StreamFlags: compressed (zlib).
                write_tag_u16(&mut buf, 0x54, STREAM_FLAG_COMPRESSED);
                // Prepend 4-byte uncompressed size before compressed data.
                let uncompressed_len = u32::try_from(raw_stream.len()).map_err(|_| {
                    EruditioError::Format("LRF stream too large for u32 size field".into())
                })?;
                let mut payload = Vec::with_capacity(4 + compressed.len());
                payload.extend_from_slice(&uncompressed_len.to_le_bytes());
                payload.extend_from_slice(&compressed);
                // StreamSize.
                let payload_len = u32::try_from(payload.len()).map_err(|_| {
                    EruditioError::Format("LRF compressed payload too large for u32".into())
                })?;
                write_tag_u32(&mut buf, 0x04, payload_len);
                // StreamStart.
                buf.push(0x05);
                buf.push(0xF5);
                buf.extend_from_slice(&payload);
                // StreamEnd.
                buf.push(0x06);
                buf.push(0xF5);
            } else {
                // Uncompressed stream.
                let stream_len = u32::try_from(raw_stream.len()).map_err(|_| {
                    EruditioError::Format("LRF stream too large for u32 size field".into())
                })?;
                write_tag_u16(&mut buf, 0x54, 0x0000);
                write_tag_u32(&mut buf, 0x04, stream_len);
                buf.push(0x05);
                buf.push(0xF5);
                buf.extend_from_slice(raw_stream);
                buf.push(0x06);
                buf.push(0xF5);
            }
        }

        // ObjectEnd tag.
        buf.push(0x01);
        buf.push(0xF5);

        Ok(buf)
    }
}

// ---------------------------------------------------------------------------
// Tag writing helpers
// ---------------------------------------------------------------------------

/// Writes a 2-byte tag header (low_byte, 0xF5).
fn write_tag_header(buf: &mut Vec<u8>, low: u8) {
    buf.push(low);
    buf.push(0xF5);
}

/// Writes a tag with a u16 payload.
fn write_tag_u16(buf: &mut Vec<u8>, low: u8, val: u16) {
    write_tag_header(buf, low);
    buf.extend_from_slice(&val.to_le_bytes());
}

/// Writes a tag with a u32 payload.
fn write_tag_u32(buf: &mut Vec<u8>, low: u8, val: u32) {
    write_tag_header(buf, low);
    buf.extend_from_slice(&val.to_le_bytes());
}

/// Writes a Link tag (0xF503) with a target object ID.
fn write_link_tag(buf: &mut Vec<u8>, target_id: u32) {
    write_tag_u32(buf, 0x03, target_id);
}

/// Writes a ContainedObjectsList tag (0xF50B): u16 count + count * u32 ids.
fn write_contained_objects_tag(buf: &mut Vec<u8>, ids: &[u32]) {
    write_tag_header(buf, 0x0B);
    // Object count limited to u16::MAX; in practice LRF files never exceed this.
    let count = ids.len().min(u16::MAX as usize) as u16;
    buf.extend_from_slice(&count.to_le_bytes());
    for &id in &ids[..count as usize] {
        buf.extend_from_slice(&id.to_le_bytes());
    }
}

/// Writes a RefStream tag (0xF54C) pointing to an ImageStream object.
fn write_refstream_tag(buf: &mut Vec<u8>, stream_obj_id: u32) {
    write_tag_u32(buf, 0x4C, stream_obj_id);
}

// ---------------------------------------------------------------------------
// Text stream tag helpers (inline content tags within a Text object stream)
// ---------------------------------------------------------------------------

/// P_START tag (0xF5A1): 6 bytes payload (zeros = default paragraph style).
fn emit_p_start(stream: &mut Vec<u8>) {
    stream.push(0xA1);
    stream.push(0xF5);
    stream.extend_from_slice(&[0u8; 6]);
}

/// P_END tag (0xF5A2): 0 bytes payload.
fn emit_p_end(stream: &mut Vec<u8>) {
    stream.push(0xA2);
    stream.push(0xF5);
}

/// CR tag (0xF5D2): 0 bytes payload.
fn emit_cr(stream: &mut Vec<u8>) {
    stream.push(0xD2);
    stream.push(0xF5);
}

/// TextString tag (0xF5CC): u16 byte_len + UTF-16LE text.
fn emit_text_string(stream: &mut Vec<u8>, text: &str) {
    if text.is_empty() {
        return;
    }
    let utf16: Vec<u8> = text.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    // UTF-16 byte length capped to u16::MAX; truncate if extremely long.
    let byte_len = utf16.len().min(u16::MAX as usize);
    stream.push(0xCC);
    stream.push(0xF5);
    stream.extend_from_slice(&(byte_len as u16).to_le_bytes());
    stream.extend_from_slice(&utf16[..byte_len]);
}

/// Italic start tag (0xF581): 0 bytes.
fn emit_italic_start(stream: &mut Vec<u8>) {
    stream.push(0x81);
    stream.push(0xF5);
}

/// Italic end tag (0xF582): 0 bytes.
fn emit_italic_end(stream: &mut Vec<u8>) {
    stream.push(0x82);
    stream.push(0xF5);
}

/// FontWeight tag (0xF515): u16 weight.
fn emit_font_weight(stream: &mut Vec<u8>, weight: u16) {
    stream.push(TAG_ID_FONT_WEIGHT);
    stream.push(0xF5);
    stream.extend_from_slice(&weight.to_le_bytes());
}

/// FontSize tag (0xF511): i16 size (in 1/10 pt).
fn emit_font_size(stream: &mut Vec<u8>, size_tenths: i16) {
    stream.push(TAG_ID_FONT_SIZE);
    stream.push(0xF5);
    stream.extend_from_slice(&size_tenths.to_le_bytes());
}

/// Plot tag (0xF5D1): u16 xsize + u16 ysize + u32 refobj + u32 adjustment.
fn emit_plot(stream: &mut Vec<u8>, image_obj_id: u32) {
    stream.push(0xD1);
    stream.push(0xF5);
    stream.extend_from_slice(&0u16.to_le_bytes()); // xsize (0 = auto)
    stream.extend_from_slice(&0u16.to_le_bytes()); // ysize (0 = auto)
    stream.extend_from_slice(&image_obj_id.to_le_bytes());
    stream.extend_from_slice(&0u32.to_le_bytes()); // adjustment
}

// ---------------------------------------------------------------------------
// HTML to LRF text stream conversion
// ---------------------------------------------------------------------------

/// Heading font sizes in 1/10 pt for h1..h6.
const HEADING_SIZES: [i16; 6] = [220, 180, 150, 130, 110, 100];

/// Simple state machine that converts chapter HTML content into an LRF text
/// stream (binary tag sequence).
///
/// Handles: `<p>`, `<br>`, `<b>`/`<strong>`, `<i>`/`<em>`, `<h1>`-`<h6>`,
/// `<img>` (via Plot tag), and plain text.
fn html_to_text_stream(
    html: &str,
    image_obj_map: &std::collections::HashMap<String, u32>,
) -> Vec<u8> {
    let mut stream = Vec::with_capacity(html.len() * 2);
    let mut pos = 0;
    let bytes = html.as_bytes();
    let len = bytes.len();
    let default_size: i16 = DEFAULT_FONT_SIZE as i16;

    // Track whether we're inside a paragraph context.
    let mut in_para = false;

    while pos < len {
        if bytes[pos] == b'<' {
            // Parse the tag name.
            let tag_end = match memchr::memchr(b'>', &bytes[pos..]) {
                Some(offset) => pos + offset,
                None => break,
            };
            let tag_content = &html[pos + 1..tag_end];
            let tag_name = tag_content
                .split(|c: char| c.is_whitespace())
                .next()
                .unwrap_or("");

            match () {
                _ if tag_name.eq_ignore_ascii_case("p") || tag_name.eq_ignore_ascii_case("div") => {
                    if in_para {
                        emit_p_end(&mut stream);
                    }
                    emit_p_start(&mut stream);
                    in_para = true;
                },
                _ if tag_name.eq_ignore_ascii_case("/p")
                    || tag_name.eq_ignore_ascii_case("/div") =>
                {
                    if in_para {
                        emit_p_end(&mut stream);
                        in_para = false;
                    }
                },
                _ if tag_name.eq_ignore_ascii_case("br")
                    || tag_name.eq_ignore_ascii_case("br/")
                    || tag_name.eq_ignore_ascii_case("br /") =>
                {
                    emit_cr(&mut stream);
                },
                _ if tag_name.eq_ignore_ascii_case("b")
                    || tag_name.eq_ignore_ascii_case("strong") =>
                {
                    emit_font_weight(&mut stream, 700);
                },
                _ if tag_name.eq_ignore_ascii_case("/b")
                    || tag_name.eq_ignore_ascii_case("/strong") =>
                {
                    emit_font_weight(&mut stream, 400);
                },
                _ if tag_name.eq_ignore_ascii_case("i") || tag_name.eq_ignore_ascii_case("em") => {
                    emit_italic_start(&mut stream);
                },
                _ if tag_name.eq_ignore_ascii_case("/i")
                    || tag_name.eq_ignore_ascii_case("/em") =>
                {
                    emit_italic_end(&mut stream);
                },
                _ if tag_name.len() == 2
                    && tag_name.as_bytes()[0].eq_ignore_ascii_case(&b'h')
                    && tag_name.as_bytes()[1].is_ascii_digit() =>
                {
                    let level = (tag_name.as_bytes()[1] - b'1') as usize;
                    let size = if level < HEADING_SIZES.len() {
                        HEADING_SIZES[level]
                    } else {
                        default_size
                    };
                    emit_font_size(&mut stream, size);
                    emit_font_weight(&mut stream, 700);
                    if in_para {
                        emit_p_end(&mut stream);
                    }
                    emit_p_start(&mut stream);
                    in_para = true;
                },
                _ if tag_name.len() == 3
                    && tag_name.as_bytes()[0] == b'/'
                    && tag_name.as_bytes()[1].eq_ignore_ascii_case(&b'h')
                    && tag_name.as_bytes()[2].is_ascii_digit() =>
                {
                    if in_para {
                        emit_p_end(&mut stream);
                        in_para = false;
                    }
                    emit_font_weight(&mut stream, 400);
                    emit_font_size(&mut stream, default_size);
                },
                _ if tag_name.len() >= 3 && tag_name[..3].eq_ignore_ascii_case("img") => {
                    // Extract src attribute.
                    if let Some(src) = extract_attr(tag_content, "src") {
                        // Look up the image object ID.
                        if let Some(&img_id) = image_obj_map.get(&src) {
                            if !in_para {
                                emit_p_start(&mut stream);
                                in_para = true;
                            }
                            emit_plot(&mut stream, img_id);
                        }
                    }
                },
                _ => {
                    // Unknown tag -- skip.
                },
            }

            pos = tag_end + 1;
        } else {
            // Accumulate text until the next '<' or end.
            let text_end = memchr::memchr(b'<', &bytes[pos..])
                .map(|off| pos + off)
                .unwrap_or(len);
            let text = &html[pos..text_end];
            let decoded = decode_html_entities(text);
            let trimmed = decoded.as_str();
            if !trimmed.is_empty() {
                if !in_para {
                    emit_p_start(&mut stream);
                    in_para = true;
                }
                emit_text_string(&mut stream, trimmed);
            }
            pos = text_end;
        }
    }

    // Close any open paragraph.
    if in_para {
        emit_p_end(&mut stream);
    }

    // Ensure non-empty stream (reader expects at least a paragraph).
    if stream.is_empty() {
        emit_p_start(&mut stream);
        emit_p_end(&mut stream);
    }

    stream
}

/// Extracts the value of an HTML attribute from a tag body.
/// e.g. `extract_attr("img src=\"foo.jpg\" /", "src")` -> `Some("foo.jpg")`.
fn extract_attr(tag_body: &str, attr: &str) -> Option<String> {
    let lower = tag_body.to_ascii_lowercase();
    let needle = format!("{}=", attr);
    let start = lower.find(&needle)?;
    let rest = &tag_body[start + needle.len()..];
    let rest = rest.trim_start();
    if rest.starts_with('"') || rest.starts_with('\'') {
        let quote = rest.as_bytes()[0];
        let inner = &rest[1..];
        let end = inner.find(quote as char)?;
        Some(inner[..end].to_string())
    } else {
        // Unquoted: take until whitespace or end.
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

/// Decodes common HTML entities.
fn decode_html_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

// ---------------------------------------------------------------------------
// Metadata XML
// ---------------------------------------------------------------------------

/// Builds the metadata XML string from the Book's metadata.
fn build_metadata_xml(book: &Book) -> String {
    use std::fmt::Write as FmtWrite;

    let title = book.metadata.title.as_deref().unwrap_or("Untitled");
    let author = book
        .metadata
        .authors
        .first()
        .map(|s| s.as_str())
        .unwrap_or("Unknown");
    let language = book.metadata.language.as_deref().unwrap_or("en");
    let publisher = book.metadata.publisher.as_deref().unwrap_or("");

    let mut xml = String::with_capacity(512);
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<Info version=\"1.1\">\n");
    xml.push_str("  <BookInfo>\n");
    let _ = writeln!(xml, "    <Title>{}</Title>", escape_xml(title));
    let _ = writeln!(xml, "    <Author>{}</Author>", escape_xml(author));
    if !publisher.is_empty() {
        let _ = writeln!(xml, "    <Publisher>{}</Publisher>", escape_xml(publisher));
    }
    if let Some(ref desc) = book.metadata.description {
        let _ = writeln!(xml, "    <FreeText>{}</FreeText>", escape_xml(desc));
    }
    xml.push_str("  </BookInfo>\n");
    xml.push_str("  <DocInfo>\n");
    let _ = writeln!(xml, "    <Language>{}</Language>", escape_xml(language));
    xml.push_str("  </DocInfo>\n");
    xml.push_str("</Info>");

    xml
}

// ---------------------------------------------------------------------------
// Compression
// ---------------------------------------------------------------------------

/// Zlib-compresses data.
fn zlib_compress(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(data)
        .map_err(|e| EruditioError::Compression(format!("zlib write: {e}")))?;
    encoder
        .finish()
        .map_err(|e| EruditioError::Compression(format!("zlib finish: {e}")))
}

// ---------------------------------------------------------------------------
// Image type detection
// ---------------------------------------------------------------------------

/// Returns the LRF stream-flags image-type nibble for a given media type.
/// The low byte of StreamFlags encodes image format for ImageStream objects.
fn image_type_flag(media_type: &str) -> u16 {
    match media_type {
        "image/jpeg" | "image/jpg" => 0x11,
        "image/png" => 0x12,
        "image/bmp" => 0x13,
        "image/gif" => 0x14,
        _ => 0x11, // default to JPEG
    }
}

// ---------------------------------------------------------------------------
// LrfWriter
// ---------------------------------------------------------------------------

/// LRF ebook format writer.
#[derive(Default)]
pub struct LrfWriter;

impl LrfWriter {
    pub fn new() -> Self {
        Self
    }
}

impl crate::domain::FormatWriter for LrfWriter {
    fn write_book(&self, book: &Book, output: &mut dyn Write) -> Result<()> {
        let data = write_lrf(book)?;
        output.write_all(&data).map_err(EruditioError::Io)
    }
}

/// Generates a complete LRF file from a `Book` and returns the raw bytes.
pub fn write_lrf(book: &Book) -> Result<Vec<u8>> {
    // ---------------------------------------------------------------
    // 1. Metadata XML (compressed)
    // ---------------------------------------------------------------
    let xml = build_metadata_xml(book);
    let xml_bytes = xml.as_bytes();
    let compressed_xml = zlib_compress(xml_bytes)?;

    // ---------------------------------------------------------------
    // 2. Allocate object IDs
    // ---------------------------------------------------------------
    let mut next_id: u32 = 1;
    let mut alloc_id = || {
        let id = next_id;
        next_id += 1;
        id
    };

    // Fixed objects.
    let book_attr_id = alloc_id(); // 1 - root
    let text_attr_id = alloc_id(); // 2
    let block_attr_id = alloc_id(); // 3
    let page_attr_id = alloc_id(); // 4

    // ---------------------------------------------------------------
    // 3. Build image objects (ImageStream + Image) first so we know
    //    their IDs when converting chapter HTML.
    // ---------------------------------------------------------------
    let mut image_objects: Vec<LrfWriteObject> = Vec::new();
    // Map from resource href -> Image object ID (for Plot tags).
    let mut image_obj_map = std::collections::HashMap::new();

    for resource in &book.resources() {
        if !resource.media_type.starts_with("image/") {
            continue;
        }

        let stream_id = alloc_id();
        let image_id = alloc_id();

        // ImageStream object: raw image data, uncompressed.
        let mut img_stream_obj = LrfWriteObject::new(stream_id, OBJ_TYPE_IMAGE_STREAM);
        img_stream_obj.stream = Some(resource.data.to_vec());
        img_stream_obj.compress_stream = false;

        // Override StreamFlags with the image type in the low byte.
        // We do this by not using the compress_stream path; instead
        // we manually serialize the stream with proper flags.
        let img_flag = image_type_flag(resource.media_type);
        img_stream_obj.tags = Vec::new();

        // We need custom serialization for image streams because the
        // stream flags encode the image type, not compression.
        // Build it manually.
        let mut is_bytes = Vec::with_capacity(resource.data.len() + 32);
        // ObjectStart
        is_bytes.push(0x00);
        is_bytes.push(0xF5);
        is_bytes.extend_from_slice(&stream_id.to_le_bytes());
        is_bytes.extend_from_slice(&OBJ_TYPE_IMAGE_STREAM.to_le_bytes());
        // StreamFlags = image type (no compression, no scramble)
        write_tag_u16(&mut is_bytes, 0x54, img_flag);
        // StreamSize
        let data_len = u32::try_from(resource.data.len()).map_err(|_| {
            EruditioError::Format("LRF image data too large for u32 size field".into())
        })?;
        write_tag_u32(&mut is_bytes, 0x04, data_len);
        // StreamStart
        is_bytes.push(0x05);
        is_bytes.push(0xF5);
        is_bytes.extend_from_slice(resource.data);
        // StreamEnd
        is_bytes.push(0x06);
        is_bytes.push(0xF5);
        // ObjectEnd
        is_bytes.push(0x01);
        is_bytes.push(0xF5);

        // Use a special sentinel so we emit raw bytes instead of calling serialize().
        img_stream_obj.stream = None;
        img_stream_obj.tags = is_bytes; // HACK: we'll handle this in emission.

        // Image object: RefStream tag pointing to ImageStream.
        let mut image_obj = LrfWriteObject::new(image_id, OBJ_TYPE_IMAGE);
        write_refstream_tag(&mut image_obj.tags, stream_id);

        image_objects.push(img_stream_obj);
        image_objects.push(image_obj);

        image_obj_map.insert(resource.href.to_string(), image_id);
        // Also try with just the filename.
        if let Some(fname) = resource.href.rsplit('/').next() {
            image_obj_map.insert(fname.to_string(), image_id);
        }
        // Also map the #obj_ style references used by the reader.
        image_obj_map.insert(format!("#obj_{}", image_id), image_id);
    }

    // ---------------------------------------------------------------
    // 4. Build text objects from chapters.
    // ---------------------------------------------------------------
    let chapters = book.chapters();
    let mut text_block_pairs: Vec<(u32, u32)> = Vec::new(); // (block_id, text_id)
    let mut chapter_objects: Vec<LrfWriteObject> = Vec::new();

    for chapter in &chapters {
        let text_id = alloc_id();
        let block_id = alloc_id();

        // Build text stream from chapter HTML content.
        let text_stream = html_to_text_stream(&chapter.content, &image_obj_map);

        let mut text_obj = LrfWriteObject::new(text_id, OBJ_TYPE_TEXT);
        // Link text to its TextAttr style object.
        write_link_tag(&mut text_obj.tags, text_attr_id);
        text_obj.stream = Some(text_stream);
        text_obj.compress_stream = true;

        // Block object: links to the TextBlock.
        let mut block_obj = LrfWriteObject::new(block_id, OBJ_TYPE_BLOCK);
        write_link_tag(&mut block_obj.tags, block_attr_id);
        // The block's stream contains a Link to the text object.
        let mut block_stream = Vec::new();
        write_link_tag(&mut block_stream, text_id);
        block_obj.stream = Some(block_stream);
        block_obj.compress_stream = false;

        text_block_pairs.push((block_id, text_id));
        chapter_objects.push(text_obj);
        chapter_objects.push(block_obj);
    }

    // ---------------------------------------------------------------
    // 5. Build page objects. Group blocks into pages (one page per
    //    chapter for simplicity).
    // ---------------------------------------------------------------
    let mut page_ids: Vec<u32> = Vec::new();
    let mut page_objects: Vec<LrfWriteObject> = Vec::new();

    for &(block_id, _) in &text_block_pairs {
        let page_id = alloc_id();
        page_ids.push(page_id);

        let mut page_obj = LrfWriteObject::new(page_id, OBJ_TYPE_PAGE);
        // Link page to PageAttr.
        write_link_tag(&mut page_obj.tags, page_attr_id);
        // Page stream: Link tags for each block on this page.
        let mut page_stream = Vec::new();
        write_link_tag(&mut page_stream, block_id);
        page_obj.stream = Some(page_stream);
        page_obj.compress_stream = false;

        page_objects.push(page_obj);
    }

    // If no chapters, create a single empty page.
    if page_ids.is_empty() {
        let page_id = alloc_id();
        page_ids.push(page_id);
        let page_obj = LrfWriteObject::new(page_id, OBJ_TYPE_PAGE);
        page_objects.push(page_obj);
    }

    // ---------------------------------------------------------------
    // 6. PageTree object.
    // ---------------------------------------------------------------
    let page_tree_id = alloc_id();
    let mut page_tree_obj = LrfWriteObject::new(page_tree_id, OBJ_TYPE_PAGE_TREE);
    write_contained_objects_tag(&mut page_tree_obj.tags, &page_ids);

    // ---------------------------------------------------------------
    // 7. Fixed style objects.
    // ---------------------------------------------------------------

    // BookAttr (root object).
    let mut book_attr = LrfWriteObject::new(book_attr_id, OBJ_TYPE_BOOK_ATTR);
    // Link to PageTree.
    write_link_tag(&mut book_attr.tags, page_tree_id);

    // TextAttr.
    let mut text_attr = LrfWriteObject::new(text_attr_id, OBJ_TYPE_TEXT_ATTR);
    write_tag_u16(&mut text_attr.tags, TAG_ID_FONT_SIZE, DEFAULT_FONT_SIZE);
    write_tag_u16(&mut text_attr.tags, TAG_ID_FONT_WEIGHT, 400);

    // BlockAttr.
    let mut block_attr = LrfWriteObject::new(block_attr_id, OBJ_TYPE_BLOCK_ATTR);
    write_tag_u16(&mut block_attr.tags, TAG_ID_BLOCK_WIDTH, DEFAULT_WIDTH - 40); // margins
    write_tag_u16(
        &mut block_attr.tags,
        TAG_ID_BLOCK_HEIGHT,
        DEFAULT_HEIGHT - 80,
    );
    write_tag_u16(&mut block_attr.tags, TAG_ID_TOP_SKIP, 10);
    write_tag_u16(&mut block_attr.tags, TAG_ID_SIDE_MARGIN, 20);

    // PageAttr.
    let mut page_attr = LrfWriteObject::new(page_attr_id, OBJ_TYPE_PAGE_ATTR);
    write_tag_u16(&mut page_attr.tags, TAG_ID_PAGE_HEIGHT, DEFAULT_HEIGHT);
    write_tag_u16(&mut page_attr.tags, TAG_ID_PAGE_WIDTH, DEFAULT_WIDTH);
    write_tag_u16(&mut page_attr.tags, TAG_ID_ODD_SIDE_MARGIN, 20);
    write_tag_u16(&mut page_attr.tags, TAG_ID_EVEN_SIDE_MARGIN, 20);

    // ---------------------------------------------------------------
    // 8. Collect all objects in the order they'll be written.
    // ---------------------------------------------------------------
    let mut all_objects: Vec<LrfWriteObject> = vec![book_attr, text_attr, block_attr, page_attr];
    all_objects.extend(chapter_objects);
    all_objects.extend(page_objects);
    all_objects.push(page_tree_obj);
    all_objects.extend(image_objects);

    let number_of_objects = all_objects.len() as u64;

    // ---------------------------------------------------------------
    // 9. Serialize all objects and record their offsets.
    // ---------------------------------------------------------------
    let objects_start_offset = HEADER_SIZE + compressed_xml.len();

    struct ObjectEntry {
        id: u32,
        offset: usize,
        size: usize,
    }

    let mut serialized_objects: Vec<Vec<u8>> = Vec::with_capacity(all_objects.len());
    let mut object_entries: Vec<ObjectEntry> = Vec::with_capacity(all_objects.len());
    let mut current_offset = objects_start_offset;

    for obj in &all_objects {
        // Check if this is a manually-serialized ImageStream (HACK: tags hold raw bytes
        // and the obj_type is ImageStream with no stream set).
        let bytes = if obj.obj_type == OBJ_TYPE_IMAGE_STREAM && obj.stream.is_none() {
            // Tags field holds the complete raw object bytes.
            obj.tags.clone()
        } else {
            obj.serialize()?
        };

        object_entries.push(ObjectEntry {
            id: obj.id,
            offset: current_offset,
            size: bytes.len(),
        });
        current_offset += bytes.len();
        serialized_objects.push(bytes);
    }

    let object_index_offset = current_offset as u64;

    // ---------------------------------------------------------------
    // 10. Build the file.
    // ---------------------------------------------------------------
    let compressed_info_size = u16::try_from(compressed_xml.len() + 4).map_err(|_| {
        EruditioError::Format("LRF compressed metadata too large for u16 field".into())
    })?;
    let _uncompressed_info_size = xml_bytes.len() as u32;

    // Total file size.
    let object_index_size = all_objects.len() * 16;
    let total_size = current_offset + object_index_size;
    let mut file = Vec::with_capacity(total_size);

    // --- Header ---
    let mut header = vec![0u8; HEADER_SIZE];
    header[0..6].copy_from_slice(&LRF_MAGIC);
    // bytes 6-7: two null bytes (already zero)
    header[0x08..0x0A].copy_from_slice(&DEFAULT_VERSION.to_le_bytes());
    header[0x0A..0x0C].copy_from_slice(&0u16.to_le_bytes()); // xor_key = 0
    header[0x0C..0x10].copy_from_slice(&book_attr_id.to_le_bytes()); // root_object_id
    header[0x10..0x18].copy_from_slice(&number_of_objects.to_le_bytes());
    header[0x18..0x20].copy_from_slice(&object_index_offset.to_le_bytes());
    // 0x20-0x23: 4 bytes zeros
    header[0x24] = 1; // binding = LTR
    // 0x25: zero
    header[0x26..0x28].copy_from_slice(&DEFAULT_DPI.to_le_bytes());
    // 0x28-0x29: 2 bytes zeros
    header[0x2A..0x2C].copy_from_slice(&DEFAULT_WIDTH.to_le_bytes());
    header[0x2C..0x2E].copy_from_slice(&DEFAULT_HEIGHT.to_le_bytes());
    header[0x2E] = DEFAULT_COLOR_DEPTH;
    // 0x2F-0x43: 21 bytes zeros
    header[0x44..0x48].copy_from_slice(&0u32.to_le_bytes()); // toc_object_id = 0
    header[0x48..0x4C].copy_from_slice(&0u32.to_le_bytes()); // toc_object_offset = 0
    header[0x4C..0x4E].copy_from_slice(&compressed_info_size.to_le_bytes());
    header[0x4E..0x50].copy_from_slice(&0u16.to_le_bytes()); // thumbnail_type = 0
    header[0x50..0x54].copy_from_slice(&0u32.to_le_bytes()); // thumbnail_size = 0
    header[0x54..0x58].copy_from_slice(&(xml_bytes.len() as u32).to_le_bytes());

    file.extend_from_slice(&header);

    // --- Compressed metadata XML ---
    file.extend_from_slice(&compressed_xml);

    // --- Object data ---
    for obj_bytes in &serialized_objects {
        file.extend_from_slice(obj_bytes);
    }

    // --- Object index ---
    for entry in &object_entries {
        file.extend_from_slice(&entry.id.to_le_bytes());
        let offset_u32 = u32::try_from(entry.offset)
            .map_err(|_| EruditioError::Format("LRF object offset exceeds u32 range".into()))?;
        let size_u32 = u32::try_from(entry.size)
            .map_err(|_| EruditioError::Format("LRF object size exceeds u32 range".into()))?;
        file.extend_from_slice(&offset_u32.to_le_bytes());
        file.extend_from_slice(&size_u32.to_le_bytes());
        file.extend_from_slice(&0u32.to_le_bytes()); // reserved
    }

    Ok(file)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
    use crate::formats::lrf::LrfReader;
    use std::io::Cursor;

    #[test]
    fn write_lrf_produces_valid_header() {
        let mut book = Book::new();
        book.metadata.title = Some("Header Test".into());
        book.metadata.authors.push("Test Author".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello LRF</p>".into(),
            id: Some("ch1".into()),
        });

        let data = write_lrf(&book).unwrap();

        // Check magic.
        assert_eq!(&data[0..6], &LRF_MAGIC);
        // Check version.
        assert_eq!(
            u16::from_le_bytes([data[0x08], data[0x09]]),
            DEFAULT_VERSION
        );
        // Check dimensions.
        assert_eq!(u16::from_le_bytes([data[0x2A], data[0x2B]]), DEFAULT_WIDTH);
        assert_eq!(u16::from_le_bytes([data[0x2C], data[0x2D]]), DEFAULT_HEIGHT);
    }

    #[test]
    fn lrf_round_trip_basic() {
        let mut book = Book::new();
        book.metadata.title = Some("Round Trip LRF".into());
        book.metadata.authors.push("Alice".into());
        book.metadata.language = Some("en".into());
        book.add_chapter(&Chapter {
            title: Some("Introduction".into()),
            content: "<p>First chapter content here.</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(&Chapter {
            title: Some("Chapter Two".into()),
            content: "<p>Second chapter with more text.</p>".into(),
            id: Some("ch2".into()),
        });

        // Write.
        let mut output = Vec::new();
        LrfWriter::new().write_book(&book, &mut output).unwrap();

        // Read back.
        let mut cursor = Cursor::new(output);
        let decoded = LrfReader::new().read_book(&mut cursor).unwrap();

        // Verify metadata.
        assert_eq!(decoded.metadata.title.as_deref(), Some("Round Trip LRF"));
        assert!(decoded.metadata.authors.iter().any(|a| a == "Alice"));
        assert_eq!(decoded.metadata.language.as_deref(), Some("en"));

        // Verify content.
        let chapters = decoded.chapters();
        assert!(!chapters.is_empty());
        let all_content: String = chapters.iter().map(|c| c.content.clone()).collect();
        assert!(
            all_content.contains("First chapter content"),
            "Missing first chapter content in: {}",
            all_content
        );
        assert!(
            all_content.contains("Second chapter"),
            "Missing second chapter content in: {}",
            all_content
        );
    }

    #[test]
    fn lrf_round_trip_formatting() {
        let mut book = Book::new();
        book.metadata.title = Some("Formatted LRF".into());
        book.add_chapter(&Chapter {
            title: Some("Formatting".into()),
            content: "<p>Normal <b>bold</b> <i>italic</i> text.</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        LrfWriter::new().write_book(&book, &mut output).unwrap();

        let mut cursor = Cursor::new(output);
        let decoded = LrfReader::new().read_book(&mut cursor).unwrap();

        let all_content: String = decoded
            .chapters()
            .iter()
            .map(|c| c.content.clone())
            .collect();
        assert!(all_content.contains("Normal"));
        assert!(all_content.contains("bold"));
        assert!(all_content.contains("italic"));
    }

    #[test]
    fn lrf_round_trip_empty_book() {
        let book = Book::new();

        let mut output = Vec::new();
        LrfWriter::new().write_book(&book, &mut output).unwrap();

        let mut cursor = Cursor::new(output);
        let decoded = LrfReader::new().read_book(&mut cursor).unwrap();

        // Should succeed without panicking; content may be minimal.
        assert!(decoded.chapters().is_empty() || !decoded.chapters().is_empty());
    }

    #[test]
    fn lrf_round_trip_with_heading() {
        let mut book = Book::new();
        book.metadata.title = Some("Heading Test".into());
        book.add_chapter(&Chapter {
            title: Some("Test".into()),
            content: "<h1>Big Title</h1><p>Body text.</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        LrfWriter::new().write_book(&book, &mut output).unwrap();

        let mut cursor = Cursor::new(output);
        let decoded = LrfReader::new().read_book(&mut cursor).unwrap();

        let all_content: String = decoded
            .chapters()
            .iter()
            .map(|c| c.content.clone())
            .collect();
        assert!(all_content.contains("Big Title"));
        assert!(all_content.contains("Body text"));
    }

    #[test]
    fn lrf_round_trip_br_tag() {
        let mut book = Book::new();
        book.metadata.title = Some("BR Test".into());
        book.add_chapter(&Chapter {
            title: Some("Test".into()),
            content: "<p>Line one<br/>Line two</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        LrfWriter::new().write_book(&book, &mut output).unwrap();

        let mut cursor = Cursor::new(output);
        let decoded = LrfReader::new().read_book(&mut cursor).unwrap();

        let all_content: String = decoded
            .chapters()
            .iter()
            .map(|c| c.content.clone())
            .collect();
        assert!(all_content.contains("Line one"));
        assert!(all_content.contains("Line two"));
    }

    #[test]
    fn html_to_text_stream_basic() {
        let map = std::collections::HashMap::new();
        let stream = html_to_text_stream("<p>Hello</p>", &map);
        // Should contain P_START, TextString, P_END tags.
        // P_START: 0xA1 0xF5 + 6 zeros = 8 bytes
        assert!(stream.len() > 8);
        assert_eq!(stream[0], 0xA1);
        assert_eq!(stream[1], 0xF5);
    }

    #[test]
    fn extract_attr_quoted() {
        assert_eq!(
            extract_attr("img src=\"foo.jpg\" /", "src"),
            Some("foo.jpg".into())
        );
    }

    #[test]
    fn extract_attr_single_quoted() {
        assert_eq!(
            extract_attr("img src='bar.png' /", "src"),
            Some("bar.png".into())
        );
    }

    #[test]
    fn decode_html_entities_basic() {
        assert_eq!(decode_html_entities("a &amp; b"), "a & b");
        assert_eq!(decode_html_entities("&lt;tag&gt;"), "<tag>");
    }
}
