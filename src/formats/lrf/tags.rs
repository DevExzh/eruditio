//! LRF binary tag (TLV) parser.
//!
//! Every piece of data inside an LRF object is encoded as a 2-byte tag
//! header (`tag_id`, `0xF5`) followed by a type-specific payload.

use super::header::read_u16_le;
use crate::error::{EruditioError, Result};

/// A parsed LRF tag.
#[derive(Debug, Clone)]
pub struct Tag {
    pub id: u16,
    pub contents: Vec<u8>,
}

impl Tag {
    /// Reads the tag payload as a u8.
    pub fn as_u8(&self) -> u8 {
        if self.contents.is_empty() {
            0
        } else {
            self.contents[0]
        }
    }

    /// Reads the tag payload as a u16 (little-endian).
    pub fn as_u16(&self) -> u16 {
        if self.contents.len() < 2 {
            return 0;
        }
        u16::from_le_bytes([self.contents[0], self.contents[1]])
    }

    /// Reads the tag payload as an i16 (little-endian).
    pub fn as_i16(&self) -> i16 {
        if self.contents.len() < 2 {
            return 0;
        }
        i16::from_le_bytes([self.contents[0], self.contents[1]])
    }

    /// Reads the tag payload as a u32 (little-endian).
    pub fn as_u32(&self) -> u32 {
        if self.contents.len() < 4 {
            return 0;
        }
        u32::from_le_bytes([
            self.contents[0],
            self.contents[1],
            self.contents[2],
            self.contents[3],
        ])
    }

    /// Reads the tag payload as a UTF-16LE string (first 2 bytes = length).
    pub fn as_string(&self) -> String {
        if self.contents.len() < 2 {
            return String::new();
        }
        let byte_len = read_u16_le(&self.contents, 0) as usize;
        let str_end = (2 + byte_len).min(self.contents.len());
        decode_utf16le(&self.contents[2..str_end])
    }

    /// Reads the tag payload as a list of u32 object IDs
    /// (first 2 bytes = count, then count × 4 bytes).
    pub fn as_object_ids(&self) -> Vec<u32> {
        if self.contents.len() < 2 {
            return Vec::new();
        }
        let count = read_u16_le(&self.contents, 0) as usize;
        let mut ids = Vec::with_capacity(count);
        for i in 0..count {
            let offset = 2 + i * 4;
            if offset + 4 <= self.contents.len() {
                ids.push(u32::from_le_bytes([
                    self.contents[offset],
                    self.contents[offset + 1],
                    self.contents[offset + 2],
                    self.contents[offset + 3],
                ]));
            }
        }
        ids
    }
}

// -- Structural tag IDs --
pub const TAG_OBJECT_START: u16 = 0xF500;
pub const TAG_OBJECT_END: u16 = 0xF501;
pub const TAG_OBJECT_INFO_LINK: u16 = 0xF502;
pub const TAG_LINK: u16 = 0xF503;
pub const TAG_STREAM_SIZE: u16 = 0xF504;
pub const TAG_STREAM_START: u16 = 0xF505;
pub const TAG_STREAM_END: u16 = 0xF506;
pub const TAG_CONTAINED_OBJECTS: u16 = 0xF50B;
pub const TAG_STREAM_FLAGS: u16 = 0xF554;
pub const TAG_REFSTREAM: u16 = 0xF54C;

// -- Text content tag IDs --
pub const TAG_TEXT_P_START: u16 = 0xF5A1;
pub const TAG_TEXT_P_END: u16 = 0xF5A2;
pub const TAG_TEXT_CR: u16 = 0xF5D2;
pub const TAG_TEXT_ITALIC_START: u16 = 0xF581;
pub const TAG_TEXT_ITALIC_END: u16 = 0xF582;
pub const TAG_TEXT_SUP_START: u16 = 0xF5B7;
pub const TAG_TEXT_SUP_END: u16 = 0xF5B8;
pub const TAG_TEXT_SUB_START: u16 = 0xF5B9;
pub const TAG_TEXT_SUB_END: u16 = 0xF5BA;
pub const TAG_TEXT_NOBR_START: u16 = 0xF5BB;
pub const TAG_TEXT_NOBR_END: u16 = 0xF5BC;
pub const TAG_TEXT_PLOT: u16 = 0xF5D1;
pub const TAG_TEXT_CR_GRAPH: u16 = 0xF5CC;
pub const TAG_TEXT_CHAR_BUTTON: u16 = 0xF5A7;
pub const TAG_TEXT_CHAR_BUTTON_END: u16 = 0xF5A8;
pub const TAG_TEXT_EMPLINE_START: u16 = 0xF5C1;
pub const TAG_TEXT_EMPLINE_END: u16 = 0xF5C2;
pub const TAG_TEXT_SPACE: u16 = 0xF5CA;

// -- Style attribute tag IDs --
pub const TAG_FONT_SIZE: u16 = 0xF511;
pub const TAG_FONT_WEIGHT: u16 = 0xF515;
pub const TAG_FONT_FACE: u16 = 0xF516;
pub const TAG_TEXT_COLOR: u16 = 0xF517;
pub const TAG_TEXT_BG_COLOR: u16 = 0xF518;
pub const TAG_LINE_SPACE: u16 = 0xF51C;
pub const TAG_PAR_INDENT: u16 = 0xF51D;
pub const TAG_ALIGN: u16 = 0xF53C;

/// Returns the payload size for a given tag ID.
/// Returns `None` for string/variable-length tags (read dynamically).
fn tag_payload_size(tag_id: u16) -> Option<usize> {
    let id = (tag_id & 0xFF) as u8;
    match id {
        // Structural tags
        0x00 => Some(6), // ObjectStart: u32 obj_id + u16 obj_type
        0x01 => Some(0), // ObjectEnd
        0x02 => Some(4), // ObjectInfoLink
        0x03 => Some(4), // Link (u32)
        0x04 => Some(4), // StreamSize (u32)
        0x05 => Some(0), // StreamStart
        0x06 => Some(0), // StreamEnd
        0x0B => None,    // ContainedObjectsList (variable: type_one)
        0x54 => Some(2), // StreamFlags (u16)
        0x7C => Some(4), // ParentPageTree (u32)

        // Style attributes (fixed-size)
        0x11 => Some(2), // FontSize (i16)
        0x12 => Some(2), // FontWidth
        0x13 => Some(4), // FontEscapement
        0x14 => Some(4), // FontOrientation
        0x15 => Some(2), // FontWeight (u16)
        0x16 => None,    // FontFaceName (string)
        0x17 => Some(4), // TextColor (u32)
        0x18 => Some(4), // TextBgColor (u32)
        0x19 => Some(2), // WordSpace
        0x1A => Some(2), // LetterSpace
        0x1B => Some(4), // BaseLineSkip
        0x1C => Some(2), // LineSpace (i16)
        0x1D => Some(2), // ParIndent (i16)
        0x1E => Some(2), // ParSkip

        // Block attributes
        0x41 => Some(2), // BlockWidth
        0x42 => Some(2), // BlockHeight
        0x43 => Some(2), // BlockRule
        0x44 => Some(4), // BgColor
        0x45 => Some(2), // Layout (columns)
        0x46 => Some(2), // FrameWidth
        0x47 => Some(4), // FrameColor
        0x48 => Some(2), // FrameMode
        0x49 => Some(2), // TopSkip
        0x4A => Some(2), // SideMargin
        0x4B => Some(2), // FootSkip
        0x4C => Some(4), // RefStream (u32 ImageStream object ID)

        // Page attributes
        0x31 => Some(2), // OddSideMargin
        0x32 => Some(2), // PageHeight
        0x33 => Some(2), // PageWidth
        0x34 => Some(4), // Unknown/Header
        0x35 => Some(4), // Unknown/Footer
        0x36 => Some(2), // EvenSideMargin

        // Alignment
        0x3C => Some(2), // Align

        // Text content tags
        0xA1 => Some(6),  // P start (style + size)
        0xA2 => Some(0),  // P end
        0xA7 => Some(4),  // CharButton (u32 refobj)
        0xA8 => Some(0),  // CharButton end
        0xA9 => Some(10), // Ruby start
        0xAA => Some(0),  // Ruby end
        0xAB => Some(0),  // Oyamoji start
        0xAC => Some(0),  // Oyamoji end
        0xAD => Some(0),  // Rubimoji start
        0xAE => Some(0),  // Rubimoji end
        0xB7 => Some(0),  // Sup start
        0xB8 => Some(0),  // Sup end
        0xB9 => Some(0),  // Sub start
        0xBA => Some(0),  // Sub end
        0xBB => Some(0),  // NoBR start
        0xBC => Some(0),  // NoBR end
        0xC1 => Some(6),  // EmpLine start
        0xC2 => Some(0),  // EmpLine end
        0xC6 => Some(10), // Box start
        0xC7 => Some(0),  // Box end
        0xCA => Some(2),  // Space (u16)
        0xCC => None,     // Text string (u16 len + UTF-16LE data)
        0xD1 => Some(12), // Plot (u16 x + u16 y + u32 refobj + u32 adj)
        0xD2 => Some(0),  // CR

        // Other common tags
        0x21 => Some(2), // RuledLine type
        0x22 => Some(2), // RuledLine width
        0x29 => Some(4), // EmpDotsPosition
        0x2A => Some(4), // EmpDotsCode
        0x2B => Some(4), // EmpLinePosition
        0x2C => Some(4), // EmpLineCode

        // Jump target tags (Button)
        0x56 => Some(8), // JumpTo (u32 page + u32 block)
        0x57 => Some(0), // Unknown/None
        0x58 => Some(0), // Unknown/None

        // Default: skip 2 bytes (best guess for unknown fixed tags)
        _ => Some(0),
    }
}

/// Parses one tag from the data at the given offset.
/// Returns the parsed tag and the new offset after the tag.
pub fn parse_tag(data: &[u8], offset: usize) -> Result<(Tag, usize)> {
    if offset + 2 > data.len() {
        return Err(EruditioError::Format(
            "LRF tag: unexpected end of data".into(),
        ));
    }

    let tag_id_low = data[offset];
    let marker = data[offset + 1];

    if marker != 0xF5 {
        return Err(EruditioError::Format(format!(
            "LRF tag: invalid marker byte 0x{:02X} at offset 0x{:X} (expected 0xF5)",
            marker, offset
        )));
    }

    let tag_id = 0xF500 | tag_id_low as u16;
    let mut pos = offset + 2;

    let contents = match tag_payload_size(tag_id) {
        Some(size) => {
            if pos + size > data.len() {
                return Err(EruditioError::Format(format!(
                    "LRF tag 0x{:04X}: payload extends past data (need {} bytes at 0x{:X})",
                    tag_id, size, pos
                )));
            }
            let c = data[pos..pos + size].to_vec();
            pos += size;
            c
        },
        None => {
            // Variable-length: string or object list.
            let low = tag_id & 0xFF;
            if low == 0x0B {
                // ContainedObjectsList: u16 count, then count × u32
                if pos + 2 > data.len() {
                    return Err(EruditioError::Format(
                        "LRF tag: ContainedObjectsList missing count".into(),
                    ));
                }
                let count = read_u16_le(data, pos) as usize;
                let total = 2 + count * 4;
                if pos + total > data.len() {
                    return Err(EruditioError::Format(
                        "LRF tag: ContainedObjectsList extends past data".into(),
                    ));
                }
                let c = data[pos..pos + total].to_vec();
                pos += total;
                c
            } else {
                // String: u16 byte_count, then that many bytes (UTF-16LE).
                if pos + 2 > data.len() {
                    return Err(EruditioError::Format(
                        "LRF tag: string missing length prefix".into(),
                    ));
                }
                let byte_len = read_u16_le(data, pos) as usize;
                let total = 2 + byte_len;
                if pos + total > data.len() {
                    return Err(EruditioError::Format(
                        "LRF tag: string extends past data".into(),
                    ));
                }
                let c = data[pos..pos + total].to_vec();
                pos += total;
                c
            }
        },
    };

    Ok((
        Tag {
            id: tag_id,
            contents,
        },
        pos,
    ))
}

/// Parses all tags from a byte slice until exhausted or ObjectEnd is hit.
pub fn parse_tags(data: &[u8]) -> Result<Vec<Tag>> {
    let mut tags = Vec::new();
    let mut offset = 0;

    while offset < data.len() {
        // Check for 0xF5 marker at next byte + 1.
        if offset + 1 >= data.len() || data[offset + 1] != 0xF5 {
            break;
        }

        let (tag, new_offset) = parse_tag(data, offset)?;
        let is_end = tag.id == TAG_OBJECT_END;
        tags.push(tag);
        offset = new_offset;

        if is_end {
            break;
        }
    }

    Ok(tags)
}

/// Decodes UTF-16LE bytes to a Rust String.
pub fn decode_utf16le(data: &[u8]) -> String {
    let u16s: Vec<u16> = data
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    String::from_utf16_lossy(&u16s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_object_start_tag() {
        // Tag 0x00, marker 0xF5, payload: obj_id=1 (u32) + obj_type=0x0A (u16)
        let data = [
            0x00, 0xF5, // tag header
            0x01, 0x00, 0x00, 0x00, // obj_id = 1
            0x0A, 0x00, // obj_type = Text
        ];
        let (tag, offset) = parse_tag(&data, 0).unwrap();
        assert_eq!(tag.id, TAG_OBJECT_START);
        assert_eq!(tag.as_u32(), 1); // obj_id
        assert_eq!(offset, 8);
    }

    #[test]
    fn parses_object_end_tag() {
        let data = [0x01, 0xF5];
        let (tag, offset) = parse_tag(&data, 0).unwrap();
        assert_eq!(tag.id, TAG_OBJECT_END);
        assert!(tag.contents.is_empty());
        assert_eq!(offset, 2);
    }

    #[test]
    fn parses_link_tag() {
        let data = [0x03, 0xF5, 0x42, 0x00, 0x00, 0x00];
        let (tag, _) = parse_tag(&data, 0).unwrap();
        assert_eq!(tag.id, TAG_LINK);
        assert_eq!(tag.as_u32(), 0x42);
    }

    #[test]
    fn rejects_invalid_marker() {
        let data = [0x00, 0xAA, 0x00, 0x00];
        let result = parse_tag(&data, 0);
        assert!(result.is_err());
    }

    #[test]
    fn decodes_utf16le_string() {
        let data = [0x48, 0x00, 0x69, 0x00]; // "Hi"
        assert_eq!(decode_utf16le(&data), "Hi");
    }

    #[test]
    fn parses_stream_flags_tag() {
        let data = [0x54, 0xF5, 0x00, 0x03]; // flags = 0x0300
        let (tag, _) = parse_tag(&data, 0).unwrap();
        assert_eq!(tag.id, TAG_STREAM_FLAGS);
        assert_eq!(tag.as_u16(), 0x0300);
    }
}
