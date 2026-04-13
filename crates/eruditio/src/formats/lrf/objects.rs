//! LRF object types and stream descrambling/decompression.
#![allow(dead_code)]

use crate::error::{EruditioError, Result};
use ahash::AHashMap as HashMap;
use flate2::bufread::ZlibDecoder;
use std::io::Read as IoRead;

use super::header::{read_u16_le, read_u32_le};
use super::tags::{
    self, TAG_CONTAINED_OBJECTS, TAG_LINK, TAG_OBJECT_START, TAG_PAGE_LIST, TAG_STREAM_END,
    TAG_STREAM_FLAGS, TAG_STREAM_SIZE, TAG_STREAM_START, Tag,
};

/// Stream flag bits.
const STREAM_COMPRESSED: u16 = 0x100;
const STREAM_SCRAMBLED: u16 = 0x200;

/// Object type IDs from the ObjectStart tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub(crate) enum ObjType {
    PageTree = 0x01,
    Page = 0x02,
    Header = 0x03,
    Footer = 0x04,
    PageAttr = 0x05,
    Block = 0x06,
    BlockAttr = 0x07,
    MiniPage = 0x08,
    Text = 0x0A,
    TextAttr = 0x0B,
    Image = 0x0C,
    Canvas = 0x0D,
    ESound = 0x0E,
    ImageStream = 0x11,
    Import = 0x12,
    Button = 0x13,
    Window = 0x14,
    PopUpWin = 0x15,
    Sound = 0x16,
    SoundStream = 0x17,
    Font = 0x19,
    ObjectInfo = 0x1A,
    BookAttr = 0x1C,
    SimpleText = 0x1D,
    TOCObject = 0x1E,
    Unknown = 0xFF,
}

impl ObjType {
    pub fn from_u16(val: u16) -> Self {
        match val {
            0x01 => Self::PageTree,
            0x02 => Self::Page,
            0x03 => Self::Header,
            0x04 => Self::Footer,
            0x05 => Self::PageAttr,
            0x06 => Self::Block,
            0x07 => Self::BlockAttr,
            0x08 => Self::MiniPage,
            0x0A => Self::Text,
            0x0B => Self::TextAttr,
            0x0C => Self::Image,
            0x0D => Self::Canvas,
            0x0E => Self::ESound,
            0x11 => Self::ImageStream,
            0x12 => Self::Import,
            0x13 => Self::Button,
            0x14 => Self::Window,
            0x15 => Self::PopUpWin,
            0x16 => Self::Sound,
            0x17 => Self::SoundStream,
            0x19 => Self::Font,
            0x1A => Self::ObjectInfo,
            0x1C => Self::BookAttr,
            0x1D => Self::SimpleText,
            0x1E => Self::TOCObject,
            _ => Self::Unknown,
        }
    }

    /// Whether this object type is a "media" type (larger streams get partial descramble).
    pub fn is_media(self) -> bool {
        matches!(self, Self::ImageStream | Self::Font | Self::SoundStream)
    }
}

/// A parsed LRF object with its tags and optional stream data.
pub(crate) struct LrfObject {
    pub id: u32,
    pub obj_type: ObjType,
    pub tags: Vec<Tag>,
    pub stream_data: Option<Vec<u8>>,
    pub stream_flags: u16,
}

impl LrfObject {
    /// Returns the contained object IDs (from ContainedObjectsList or PageList tag).
    /// Returns the first matching tag; LRF objects have at most one of these.
    pub fn contained_object_ids(&self) -> Vec<u32> {
        for tag in &self.tags {
            if tag.id == TAG_CONTAINED_OBJECTS || tag.id == TAG_PAGE_LIST {
                return tag.as_object_ids();
            }
        }
        Vec::new()
    }

    /// Returns the linked object ID (from Link tag), if present.
    pub fn link_id(&self) -> Option<u32> {
        self.tags
            .iter()
            .find(|t| t.id == TAG_LINK)
            .map(|t| t.as_u32())
    }
}

/// Parses all objects from the LRF file data using the object index.
pub(crate) fn parse_objects(
    data: &[u8],
    object_index_offset: u64,
    number_of_objects: u64,
    xor_key: u16,
) -> Result<HashMap<u32, LrfObject>> {
    let idx_start = usize::try_from(object_index_offset)
        .map_err(|_| EruditioError::Format("LRF object index offset too large".into()))?;
    let entry_size = 16; // 4 × u32
    let idx_end = usize::try_from(number_of_objects)
        .ok()
        .and_then(|n| n.checked_mul(entry_size))
        .and_then(|n| n.checked_add(idx_start))
        .ok_or_else(|| EruditioError::Format("LRF object index size overflow".into()))?;

    if idx_end > data.len() {
        return Err(EruditioError::Format(
            "LRF object index extends past end of file".into(),
        ));
    }

    // Cap allocation to prevent OOM from crafted number_of_objects.
    let cap = (number_of_objects as usize).min(data.len() / entry_size);
    let mut objects = HashMap::with_capacity(cap);

    for i in 0..number_of_objects as usize {
        let entry_offset = idx_start + i * entry_size;
        let obj_id = read_u32_le(data, entry_offset);
        let obj_offset = read_u32_le(data, entry_offset + 4) as usize;
        let obj_size = read_u32_le(data, entry_offset + 8) as usize;

        if obj_offset + obj_size > data.len() {
            continue; // Skip corrupt entries rather than failing entirely.
        }

        match parse_single_object(data, obj_offset, obj_size, xor_key) {
            Ok(obj) => {
                objects.insert(obj_id, obj);
            },
            Err(_) => continue, // Skip unparseable objects.
        }
    }

    Ok(objects)
}

/// Parses a single object from its byte range.
fn parse_single_object(data: &[u8], offset: usize, size: usize, xor_key: u16) -> Result<LrfObject> {
    let obj_data = &data[offset..offset + size];

    // Parse tags from the object data.
    let all_tags = tags::parse_tags(obj_data)?;

    if all_tags.is_empty() {
        return Err(EruditioError::Format("LRF object has no tags".into()));
    }

    // First tag must be ObjectStart.
    let start_tag = &all_tags[0];
    if start_tag.id != TAG_OBJECT_START || start_tag.contents.len() < 6 {
        return Err(EruditioError::Format(
            "LRF object missing ObjectStart tag".into(),
        ));
    }

    let obj_id = read_u32_le(&start_tag.contents, 0);
    let obj_type_raw = read_u16_le(&start_tag.contents, 4);
    let obj_type = ObjType::from_u16(obj_type_raw);

    // Extract stream data and flags from tags.
    let mut stream_flags: u16 = 0;
    let mut stream_size: u32 = 0;
    let mut stream_data: Option<Vec<u8>> = None;
    let mut non_stream_tags = Vec::new();
    let mut in_stream = false;

    for tag in all_tags {
        if in_stream {
            if tag.id == TAG_STREAM_END {
                in_stream = false;
            }
            continue;
        }
        match tag.id {
            TAG_STREAM_FLAGS => stream_flags = tag.as_u16(),
            TAG_STREAM_SIZE => stream_size = tag.as_u32(),
            TAG_STREAM_START => {
                // The stream data follows this tag in the raw bytes.
                // Find the byte offset of this tag, then read stream_size bytes.
                let stream_bytes = extract_stream_bytes(obj_data, stream_size as usize);
                if let Some(raw) = stream_bytes {
                    stream_data = Some(process_stream(
                        &raw,
                        stream_flags,
                        xor_key,
                        obj_type.is_media(),
                    )?);
                }
                in_stream = true;
            },
            TAG_STREAM_END => {},
            _ => non_stream_tags.push(tag),
        }
    }

    Ok(LrfObject {
        id: obj_id,
        obj_type,
        tags: non_stream_tags,
        stream_data,
        stream_flags,
    })
}

/// Extracts raw stream bytes from object data by finding the StreamStart tag
/// and reading `size` bytes after it.
fn extract_stream_bytes(obj_data: &[u8], size: usize) -> Option<Vec<u8>> {
    // Scan for the StreamStart marker: 0x05 0xF5
    let mut pos = 0;
    while pos + 1 < obj_data.len() {
        if obj_data[pos] == 0x05 && obj_data[pos + 1] == 0xF5 {
            let start = pos + 2;
            let end = (start + size).min(obj_data.len());
            return Some(obj_data[start..end].to_vec());
        }
        pos += 1;
    }
    None
}

/// Descrambles and decompresses a stream according to its flags.
fn process_stream(raw: &[u8], flags: u16, xor_key: u16, is_media: bool) -> Result<Vec<u8>> {
    let mut buf = raw.to_vec();

    // Step 1: Descramble if flag 0x200 is set.
    if flags & STREAM_SCRAMBLED != 0 {
        descramble(&mut buf, xor_key, is_media);
    }

    // Step 2: Decompress if flag 0x100 is set.
    if flags & STREAM_COMPRESSED != 0 {
        if buf.len() < 4 {
            return Err(EruditioError::Compression(
                "LRF compressed stream too short for size prefix".into(),
            ));
        }
        let decomp_size = read_u32_le(&buf, 0) as u64;
        // Cap decompression output to prevent decompression bombs.
        const MAX_DECOMPRESS: u64 = 256 * 1024 * 1024; // 256 MB
        let limit = decomp_size.min(MAX_DECOMPRESS);
        let compressed = &buf[4..];
        let decoder = ZlibDecoder::new(compressed);
        let mut limited = decoder.take(limit);
        let mut output = Vec::new();
        limited.read_to_end(&mut output).map_err(|e| {
            EruditioError::Compression(format!("LRF stream decompression error: {}", e))
        })?;
        buf = output;
    }

    Ok(buf)
}

/// XOR-based stream descrambling.
fn descramble(buf: &mut [u8], xor_key: u16, is_media: bool) {
    let length = buf.len();
    let raw_key = (xor_key & 0xFF) as usize;

    let key = if raw_key != 0 && raw_key <= 0xF0 {
        (length % raw_key + 0x0F) as u8
    } else {
        0u8
    };

    if key == 0 {
        return;
    }

    // For large media objects, only descramble first 0x400 bytes.
    let limit = if length > 0x400 && is_media {
        0x400
    } else {
        length
    };

    for byte in buf[..limit].iter_mut() {
        *byte ^= key;
    }
}

/// Parses TOC entries from a TOCObject's stream data.
pub(crate) struct TocEntry {
    pub refpage: u32,
    pub refobj: u32,
    pub label: String,
}

pub(crate) fn parse_toc_stream(stream: &[u8]) -> Result<Vec<TocEntry>> {
    if stream.len() < 4 {
        return Ok(Vec::new());
    }

    let count = read_u32_le(stream, 0) as usize;
    // Skip offset table: count × 4 bytes (one u32 offset per entry).
    let mut pos = count
        .checked_mul(4)
        .and_then(|n| n.checked_add(4))
        .filter(|&n| n <= stream.len())
        .unwrap_or(stream.len());

    let cap = count.min(stream.len() / 10); // Each entry is ≥10 bytes
    let mut entries = Vec::with_capacity(cap);
    for _ in 0..count {
        if pos + 10 > stream.len() {
            break;
        }
        let refpage = read_u32_le(stream, pos);
        let refobj = read_u32_le(stream, pos + 4);
        let label_bytes = read_u16_le(stream, pos + 8) as usize;
        pos += 10;

        let label_end = (pos + label_bytes).min(stream.len());
        let label = tags::decode_utf16le(&stream[pos..label_end]);
        pos = label_end;

        entries.push(TocEntry {
            refpage,
            refobj,
            label,
        });
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descramble_xor_key_zero_is_noop() {
        let mut buf = vec![0x41, 0x42, 0x43];
        descramble(&mut buf, 0, false);
        assert_eq!(buf, vec![0x41, 0x42, 0x43]);
    }

    #[test]
    fn descramble_applies_xor() {
        let mut buf = vec![0x00, 0x00, 0x00, 0x00];
        // key = (xor_key & 0xFF) = 0x10
        // derived key = 4 % 0x10 + 0x0F = 4 + 15 = 19 = 0x13
        descramble(&mut buf, 0x10, false);
        assert_eq!(buf, vec![0x13, 0x13, 0x13, 0x13]);
    }

    #[test]
    fn descramble_media_limits_to_0x400() {
        let mut buf = vec![0x00; 0x500];
        descramble(&mut buf, 0x10, true);
        // First 0x400 bytes should be XORed.
        assert_ne!(buf[0], 0x00);
        // Bytes after 0x400 should be untouched.
        assert_eq!(buf[0x400], 0x00);
    }

    #[test]
    fn obj_type_round_trip() {
        assert_eq!(ObjType::from_u16(0x0A), ObjType::Text);
        assert_eq!(ObjType::from_u16(0x01), ObjType::PageTree);
        assert_eq!(ObjType::from_u16(0x1E), ObjType::TOCObject);
        assert_eq!(ObjType::from_u16(0xFF), ObjType::Unknown);
    }

    #[test]
    fn parse_toc_stream_basic() {
        // Build a minimal TOC stream: 1 entry
        let mut stream = Vec::new();
        stream.extend_from_slice(&1u32.to_le_bytes()); // count = 1 (u32)
        // Offset table: count × u32 = 4 bytes (one offset entry)
        stream.extend_from_slice(&[0u8; 4]);
        // Entry: refpage=1, refobj=2, label="Ch1" (UTF-16LE)
        stream.extend_from_slice(&1u32.to_le_bytes());
        stream.extend_from_slice(&2u32.to_le_bytes());
        let label_utf16: Vec<u8> = "Ch1".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        stream.extend_from_slice(&(label_utf16.len() as u16).to_le_bytes());
        stream.extend_from_slice(&label_utf16);

        let entries = parse_toc_stream(&stream).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].refpage, 1);
        assert_eq!(entries[0].refobj, 2);
        assert_eq!(entries[0].label, "Ch1");
    }
}
