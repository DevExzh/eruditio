//! LIT (Microsoft Reader) format writer.
//!
//! Produces a valid ITOLITLS container with uncompressed content (section 0
//! only). This avoids LZX compression while still producing a LIT file that
//! compliant readers (including eruditio's own reader) can open.
//!
//! Binary HTML encoding is the inverse of `unbinary.rs`: we parse HTML/OPF
//! with `quick_xml` and emit the binary token stream using the same tag and
//! attribute code tables from `maps.rs`.

use std::collections::HashMap;
use std::io::Write;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::formats::common::text_utils::{bytes_to_cow_str, escape_xml};

use crate::domain::{Book, FormatWriter};
use crate::error::{EruditioError, Result};

use super::maps::{self, HTML_GLOBAL_ATTRS, HTML_TAGS, OPF_ATTRS, OPF_TAGS};
use super::msdes::MsDes;
use super::mssha1::MsSha1;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// ITOLITLS primary header magic.
const MAGIC: &[u8; 8] = b"ITOLITLS";

/// File GUID: {0A9007C1-4076-11D3-8789-0000F8105754}
const FILE_GUID: [u8; 16] = [
    0xC1, 0x07, 0x90, 0x0A, 0x76, 0x40, 0xD3, 0x11, 0x87, 0x89, 0x00, 0x00, 0xF8, 0x10, 0x57, 0x54,
];

/// Piece 3 GUID: {0A9007C3-4076-11D3-8789-0000F8105754}
const PIECE3_GUID: [u8; 16] = [
    0xC3, 0x07, 0x90, 0x0A, 0x76, 0x40, 0xD3, 0x11, 0x87, 0x89, 0x00, 0x00, 0xF8, 0x10, 0x57, 0x54,
];

/// Piece 4 GUID: {0A9007C4-4076-11D3-8789-0000F8105754}
const PIECE4_GUID: [u8; 16] = [
    0xC4, 0x07, 0x90, 0x0A, 0x76, 0x40, 0xD3, 0x11, 0x87, 0x89, 0x00, 0x00, 0xF8, 0x10, 0x57, 0x54,
];

/// AOLL chunk size (directory chunk). Must match what the reader expects.
/// Using 0x2000 (8192 bytes) which is a common LIT chunk size.
const CHUNK_SIZE: usize = 0x2000;

/// Binary token flag bits (matching unbinary.rs).
const FLAG_OPENING: u8 = 0x01;
const FLAG_CLOSING: u8 = 0x02;

// ---------------------------------------------------------------------------
// Reverse maps: tag name -> index, attr name -> code
// ---------------------------------------------------------------------------

/// Build a reverse map from HTML tag name to its index in HTML_TAGS.
fn build_html_tag_reverse() -> HashMap<&'static str, usize> {
    let mut map = HashMap::new();
    for (i, entry) in HTML_TAGS.iter().enumerate() {
        if let Some(name) = entry {
            map.insert(*name, i);
        }
    }
    map
}

/// Build a reverse map from OPF tag name to its index in OPF_TAGS.
fn build_opf_tag_reverse() -> HashMap<&'static str, usize> {
    let mut map = HashMap::new();
    for (i, entry) in OPF_TAGS.iter().enumerate() {
        if let Some(name) = entry {
            map.insert(*name, i);
        }
    }
    map
}

/// Build a reverse map from HTML global attribute name to its code.
fn build_html_global_attr_reverse() -> HashMap<&'static str, u16> {
    let mut map = HashMap::new();
    for &(code, name) in HTML_GLOBAL_ATTRS {
        map.entry(name).or_insert(code);
    }
    map
}

/// Build a reverse map from OPF attribute name to its code.
fn build_opf_attr_reverse() -> HashMap<&'static str, u16> {
    let mut map = HashMap::new();
    for &(code, name) in OPF_ATTRS {
        if !name.starts_with('%') {
            map.entry(name).or_insert(code);
        }
    }
    map
}

/// Build a reverse map from (tag_index, attr_name) to attr code for HTML.
/// This collects all per-tag attribute tables.
fn build_html_per_tag_attr_reverse() -> HashMap<(usize, &'static str), u16> {
    let mut map = HashMap::new();
    // We probe every possible tag index (0..109) for every code, but that's
    // impractical. Instead, we iterate using the forward lookup function over
    // known attribute tables.
    // The approach: for each tag index, try to look up attributes by checking
    // all known attribute codes. But that's also not great.
    //
    // Better: directly collect from the static attribute tables.
    collect_per_tag_attrs(&mut map);
    map
}

/// Type alias for the per-tag attribute table entries used by the LIT binary encoder.
type AttrTableEntry = (&'static [usize], &'static [(u16, &'static str)]);

/// Collect per-tag attribute entries from all known static tables.
fn collect_per_tag_attrs(map: &mut HashMap<(usize, &'static str), u16>) {
    // (tag_indices, table) pairs
    let tables: &[AttrTableEntry] = &[
        (&[3], maps::ATTRS_A),
        (&[5], maps::ATTRS_ADDRESS),
        (&[6], maps::ATTRS_APPLET),
        (&[7], maps::ATTRS_AREA),
        (
            &[
                8, 13, 29, 34, 51, 56, 78, 82, 83, 86, 87, 88, 89, 91, 92, 103, 104, 106,
            ],
            maps::ATTRS_STYLE_CLASS_ID,
        ),
        (&[9], maps::ATTRS_BASE),
        (&[10], maps::ATTRS_BASEFONT),
        (&[12], maps::ATTRS_BGSOUND),
        (&[15, 17, 20, 77], maps::ATTRS_CLEAR_STYLE),
        (&[16], maps::ATTRS_BODY),
        (&[18], maps::ATTRS_BUTTON),
        (&[19], maps::ATTRS_CAPTION),
        (&[21, 22], maps::ATTRS_STYLE_CLASS_ID),
        (&[23, 24], maps::ATTRS_COL),
        (&[27, 33], maps::ATTRS_DD),
        (&[31], maps::ATTRS_DIV),
        (&[32], maps::ATTRS_DL),
        (&[35], maps::ATTRS_EMBED),
        (&[36], maps::ATTRS_FIELDSET),
        (&[37], maps::ATTRS_FONT),
        (&[38], maps::ATTRS_FORM),
        (&[39], maps::ATTRS_FRAME),
        (&[40], maps::ATTRS_FRAMESET),
        (&[42, 43, 44, 45, 46, 47], maps::ATTRS_HEADING),
        (&[49], maps::ATTRS_HR),
        (&[52], maps::ATTRS_IFRAME),
        (&[53], maps::ATTRS_IMG),
        (&[54], maps::ATTRS_INPUT),
        (&[57], maps::ATTRS_LABEL),
        (&[58], maps::ATTRS_LEGEND),
        (&[59], maps::ATTRS_LI),
        (&[60], maps::ATTRS_LINK),
        (&[61], maps::ATTRS_TAG61),
        (&[62], maps::ATTRS_MAP),
        (&[63], maps::ATTRS_TAG63),
        (&[65], maps::ATTRS_META),
        (&[66], maps::ATTRS_NEXTID),
        (&[71], maps::ATTRS_OBJECT),
        (&[72], maps::ATTRS_OL),
        (&[73], maps::ATTRS_OPTION),
        (&[74], maps::ATTRS_P),
        (&[75], maps::ATTRS_PARAM),
        (&[76], maps::ATTRS_PLAINTEXT),
        (&[84], maps::ATTRS_SCRIPT),
        (&[85], maps::ATTRS_SELECT),
        (&[90], maps::ATTRS_STYLE),
        (&[93], maps::ATTRS_TABLE),
        (&[94, 98, 100], maps::ATTRS_TBODY),
        (&[95], maps::ATTRS_TC),
        (&[96, 99], maps::ATTRS_TD),
        (&[97], maps::ATTRS_TEXTAREA),
        (&[102], maps::ATTRS_TR),
        (&[105], maps::ATTRS_UL),
        (&[108], maps::ATTRS_WBR),
    ];

    for (tag_indices, table) in tables {
        for &tag_idx in *tag_indices {
            for &(code, name) in *table {
                if !name.starts_with('%') {
                    map.entry((tag_idx, name)).or_insert(code);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Variable-length integer encoding (decint)
// ---------------------------------------------------------------------------

/// Encode a value as a variable-length integer (decint).
/// High bit on each byte signals continuation; final byte has high bit clear.
fn encode_decint(value: u64) -> Vec<u8> {
    if value == 0 {
        return vec![0];
    }
    let mut bytes = Vec::new();
    let mut v = value;
    while v > 0 {
        bytes.push((v & 0x7F) as u8);
        v >>= 7;
    }
    bytes.reverse();
    let len = bytes.len();
    for b in bytes.iter_mut().take(len - 1) {
        *b |= 0x80;
    }
    bytes
}

// ---------------------------------------------------------------------------
// UTF-8 ordinal encoding (for binary token values)
// ---------------------------------------------------------------------------

/// Encode a u32 value as a UTF-8 character (matching the read_utf8_char decoder).
fn encode_utf8_ordinal(value: u32) -> Vec<u8> {
    if let Some(c) = char::from_u32(value) {
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        s.as_bytes().to_vec()
    } else {
        // Fallback: encode as replacement character
        vec![0xEF, 0xBF, 0xBD]
    }
}

// ---------------------------------------------------------------------------
// Binary HTML/OPF encoder
// ---------------------------------------------------------------------------

/// Context for binary encoding, holding reverse maps.
struct BinaryEncoder {
    tag_reverse: HashMap<&'static str, usize>,
    global_attr_reverse: HashMap<&'static str, u16>,
    per_tag_attr_reverse: HashMap<(usize, &'static str), u16>,
    /// Maps href paths to manifest internal IDs (for href/src encoding).
    href_to_id: HashMap<String, String>,
    is_html: bool,
}

impl BinaryEncoder {
    fn new_html(href_to_id: HashMap<String, String>) -> Self {
        Self {
            tag_reverse: build_html_tag_reverse(),
            global_attr_reverse: build_html_global_attr_reverse(),
            per_tag_attr_reverse: build_html_per_tag_attr_reverse(),
            href_to_id,
            is_html: true,
        }
    }

    fn new_opf() -> Self {
        Self {
            tag_reverse: build_opf_tag_reverse(),
            global_attr_reverse: build_opf_attr_reverse(),
            per_tag_attr_reverse: HashMap::new(),
            href_to_id: HashMap::new(),
            is_html: false,
        }
    }

    /// Encode an HTML or OPF document into the LIT binary token format.
    fn encode(&self, xml: &str) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        let mut reader = Reader::from_str(xml);
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    self.encode_start_tag(e, false, &mut output)?;
                },
                Ok(Event::Empty(ref e)) => {
                    self.encode_start_tag(e, true, &mut output)?;
                },
                Ok(Event::End(ref e)) => {
                    self.encode_end_tag(e, &mut output);
                },
                Ok(Event::Text(ref e)) => {
                    let bytes = e.clone().into_inner();
                    // Fast path: if all bytes are valid UTF-8, scan for NUL
                    // directly in the byte buffer instead of iterating chars.
                    if memchr::memchr(0, &bytes).is_none() {
                        // No NUL bytes -- emit raw bytes directly.
                        // For valid UTF-8, bytes_to_cow_str avoids allocation.
                        output.extend_from_slice(&bytes);
                    } else {
                        // Rare path: contains NUL bytes that need replacement.
                        let text = bytes_to_cow_str(&bytes);
                        for c in text.chars() {
                            if c == '\0' {
                                output.push(0x0B);
                            } else {
                                let mut tmp = [0u8; 4];
                                let s = c.encode_utf8(&mut tmp);
                                output.extend_from_slice(s.as_bytes());
                            }
                        }
                    }
                },
                Ok(Event::CData(ref e)) => {
                    // CData content is raw bytes -- copy directly.
                    output.extend_from_slice(e.as_ref());
                },
                Ok(Event::Eof) => break,
                Err(e) => {
                    return Err(EruditioError::Format(format!(
                        "XML parse error during binary encoding: {e}"
                    )));
                },
                _ => {}, // Skip comments, PI, declarations
            }
            buf.clear();
        }

        Ok(output)
    }

    fn encode_start_tag(
        &self,
        e: &quick_xml::events::BytesStart<'_>,
        is_empty: bool,
        output: &mut Vec<u8>,
    ) -> Result<()> {
        let tag_name_raw = e.name();
        let tag_name = bytes_to_cow_str(tag_name_raw.as_ref());
        let tag_name_lower = tag_name.to_ascii_lowercase();

        // Look up tag index
        let tag_index = self
            .tag_reverse
            .get(tag_name.as_ref())
            .or_else(|| self.tag_reverse.get(tag_name_lower.as_str()));

        // Flags: OPENING, and if empty (self-closing), also CLOSING
        let flags = if is_empty {
            FLAG_OPENING | FLAG_CLOSING
        } else {
            FLAG_OPENING
        };

        // Emit: 0x00, flags, tag_index_or_custom
        output.push(0x00);
        output.extend_from_slice(&encode_utf8_ordinal(u32::from(flags)));

        if let Some(&idx) = tag_index {
            output.extend_from_slice(&encode_utf8_ordinal(idx as u32));

            // Encode attributes
            for attr in e.attributes().flatten() {
                let attr_name = bytes_to_cow_str(attr.key.as_ref());
                let attr_name_lower = attr_name.to_ascii_lowercase();
                let attr_value = bytes_to_cow_str(&attr.value);

                let attr_code = self.find_attr_code(idx, &attr_name, &attr_name_lower);

                if let Some(code) = attr_code {
                    let is_href =
                        self.is_html && (attr_name_lower == "href" || attr_name_lower == "src");

                    output.extend_from_slice(&encode_utf8_ordinal(u32::from(code)));

                    if is_href {
                        self.encode_href_value(&attr_value, output);
                    } else {
                        self.encode_string_value(&attr_value, output);
                    }
                } else {
                    // Custom attribute: emit 0x8000 prefix, then attr name as
                    // length-prefixed string, then value
                    output.extend_from_slice(&encode_utf8_ordinal(0x8000));
                    let name_chars: Vec<char> = attr_name.chars().collect();
                    output.extend_from_slice(&encode_utf8_ordinal(name_chars.len() as u32 + 1));
                    for &c in &name_chars {
                        let mut tmp = [0u8; 4];
                        let s = c.encode_utf8(&mut tmp);
                        output.extend_from_slice(s.as_bytes());
                    }
                    // Custom attrs always use regular value encoding
                    self.encode_string_value(&attr_value, output);
                }
            }

            // End of attributes
            output.extend_from_slice(&encode_utf8_ordinal(0));
        } else {
            // Custom tag: emit 0x8000, then tag name as length-prefixed string
            output.extend_from_slice(&encode_utf8_ordinal(0x8000));
            let name_chars: Vec<char> = tag_name.chars().collect();
            output.extend_from_slice(&encode_utf8_ordinal(name_chars.len() as u32 + 1));
            for &c in &name_chars {
                let mut tmp = [0u8; 4];
                let s = c.encode_utf8(&mut tmp);
                output.extend_from_slice(s.as_bytes());
            }

            // Encode attributes (all as custom since we don't know the tag)
            for attr in e.attributes().flatten() {
                let attr_name = bytes_to_cow_str(attr.key.as_ref());
                let attr_value = bytes_to_cow_str(&attr.value);

                // Try global attr lookup first
                let attr_name_lower = attr_name.to_ascii_lowercase();
                if let Some(&code) = self.global_attr_reverse.get(attr_name_lower.as_str()) {
                    output.extend_from_slice(&encode_utf8_ordinal(u32::from(code)));
                    self.encode_string_value(&attr_value, output);
                } else {
                    // Custom attribute
                    output.extend_from_slice(&encode_utf8_ordinal(0x8000));
                    let name_chars: Vec<char> = attr_name.chars().collect();
                    output.extend_from_slice(&encode_utf8_ordinal(name_chars.len() as u32 + 1));
                    for &c in &name_chars {
                        let mut tmp = [0u8; 4];
                        let s = c.encode_utf8(&mut tmp);
                        output.extend_from_slice(s.as_bytes());
                    }
                    self.encode_string_value(&attr_value, output);
                }
            }

            // End of attributes
            output.extend_from_slice(&encode_utf8_ordinal(0));
        }

        Ok(())
    }

    fn encode_end_tag(&self, _e: &quick_xml::events::BytesEnd<'_>, output: &mut Vec<u8>) {
        // Emit closing tag token: 0x00, FLAG_CLOSING, tag_index
        // The tag_index is consumed but not used by the decoder for closing-only
        // tokens. We use 1 as a placeholder (any non-zero value works).
        output.push(0x00);
        output.extend_from_slice(&encode_utf8_ordinal(u32::from(FLAG_CLOSING)));
        output.extend_from_slice(&encode_utf8_ordinal(1));
    }

    /// Encode a regular string attribute value.
    fn encode_string_value(&self, value: &str, output: &mut Vec<u8>) {
        let chars: Vec<char> = value.chars().collect();
        let len = chars.len();
        if len == 0 {
            // Length of 1 means 0 chars (length = count + 1)
            output.extend_from_slice(&encode_utf8_ordinal(1));
        } else {
            output.extend_from_slice(&encode_utf8_ordinal(len as u32 + 1));
            for &c in &chars {
                let mut tmp = [0u8; 4];
                let s = c.encode_utf8(&mut tmp);
                output.extend_from_slice(s.as_bytes());
            }
        }
    }

    /// Encode an href/src attribute value in the LIT binary href format.
    fn encode_href_value(&self, value: &str, output: &mut Vec<u8>) {
        // The href format is: length_prefix, then type_byte + target_string
        // The decoder does:
        //   body = href_buf[1..]
        //   (doc, frag) = split on '#'
        //   resolved = item_path(doc, dir, manifest)
        //
        // So we encode: '\x01' prefix + manifest_id (or raw path if no mapping).
        let (path_part, fragment) = value
            .split_once('#')
            .map_or((value, None), |(p, f)| (p, Some(f)));

        // Try to resolve path back to manifest internal ID
        let target_id = self
            .href_to_id
            .get(path_part)
            .map(|s| s.as_str())
            .unwrap_or(path_part);

        let mut href_body = String::from('\x01');
        href_body.push_str(target_id);
        if let Some(frag) = fragment {
            href_body.push('#');
            href_body.push_str(frag);
        }

        let chars: Vec<char> = href_body.chars().collect();
        let len = chars.len();
        output.extend_from_slice(&encode_utf8_ordinal(len as u32 + 1));
        for &c in &chars {
            let mut tmp = [0u8; 4];
            let s = c.encode_utf8(&mut tmp);
            output.extend_from_slice(s.as_bytes());
        }
    }

    /// Find the attribute code for a given tag index and attribute name.
    fn find_attr_code(
        &self,
        tag_index: usize,
        attr_name: &str,
        attr_name_lower: &str,
    ) -> Option<u16> {
        if self.is_html {
            // Try per-tag first, then global
            self.per_tag_attr_reverse
                .get(&(tag_index, attr_name_lower))
                .copied()
                .or_else(|| self.global_attr_reverse.get(attr_name_lower).copied())
        } else {
            // OPF: global only
            self.global_attr_reverse
                .get(attr_name)
                .or_else(|| self.global_attr_reverse.get(attr_name_lower))
                .copied()
        }
    }
}

// ---------------------------------------------------------------------------
// Virtual filesystem entry
// ---------------------------------------------------------------------------

/// A virtual file in the LIT container, to be placed in section 0 (Uncompressed).
struct VfsEntry {
    name: String,
    section: u32,
    data: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Manifest builder
// ---------------------------------------------------------------------------

/// Build the binary manifest entry.
///
/// Format (from the reader's `read_manifest`):
///   root_len(u8) + root_string +
///   for each state ("spine", "not spine", "css", "images"):
///     num_files(i32_le) +
///     for each file:
///       offset(u32_le) +
///       sized_utf8_string(internal) +
///       sized_utf8_string(original_path) +
///       sized_utf8_string_zpad(mime_type)
fn build_manifest(
    spine_items: &[(String, String, String)], // (internal_id, path, mime_type)
    non_spine_items: &[(String, String, String)], // (internal_id, path, mime_type)
) -> Vec<u8> {
    let mut out = Vec::new();

    // Root string: "/" (1 byte length)
    let root = "/";
    out.push(root.len() as u8);
    out.extend_from_slice(root.as_bytes());

    // 4 states: spine, not spine, css, images
    // For simplicity, put all spine items in "spine" and everything else in "not spine"
    let states: [&[(String, String, String)]; 4] = [spine_items, non_spine_items, &[], &[]];

    let mut offset_counter: u32 = 0;
    for items in &states {
        out.extend_from_slice(&(items.len() as i32).to_le_bytes());
        for (internal, path, mime_type) in *items {
            out.extend_from_slice(&offset_counter.to_le_bytes());
            offset_counter = offset_counter.wrapping_add(1);

            // sized_utf8_string: first char = length, then chars
            write_sized_utf8_string(&mut out, internal, false);
            write_sized_utf8_string(&mut out, path, false);
            write_sized_utf8_string(&mut out, mime_type, true);
        }
    }

    out
}

/// Write a sized UTF-8 string: first char's ordinal = length, then that many chars.
/// If `zpad`, append a zero byte after the string.
fn write_sized_utf8_string(out: &mut Vec<u8>, s: &str, zpad: bool) {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    // Length as a UTF-8 encoded ordinal
    out.extend_from_slice(&encode_utf8_ordinal(len as u32));
    for &c in &chars {
        let mut tmp = [0u8; 4];
        let cs = c.encode_utf8(&mut tmp);
        out.extend_from_slice(cs.as_bytes());
    }
    if zpad {
        out.push(0);
    }
}

// ---------------------------------------------------------------------------
// NameList builder
// ---------------------------------------------------------------------------

/// Build the ::DataSpace/NameList entry.
///
/// Format (from the reader):
///   u16_le(?) + u16_le(num_sections) +
///   for each section:
///     u16_le(char_count) + utf16le_chars + utf16le_null
fn build_namelist(section_names: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // Unknown field
    out.extend_from_slice(&(section_names.len() as u16).to_le_bytes());

    for name in section_names {
        let chars: Vec<u16> = name.encode_utf16().collect();
        out.extend_from_slice(&(chars.len() as u16).to_le_bytes());
        for &c in &chars {
            out.extend_from_slice(&c.to_le_bytes());
        }
        // Null terminator (UTF-16LE)
        out.extend_from_slice(&0u16.to_le_bytes());
    }

    out
}

// ---------------------------------------------------------------------------
// DRM storage for free books
// ---------------------------------------------------------------------------

/// Build DRM storage entries for a free (unencrypted) book.
///
/// Returns (drm_source_data, drm_sealed_data, validation_data).
fn build_free_drm(meta_binary: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    // DRMSource: "Free as in freedom" encoded as UTF-16LE null-terminated
    let source_text = "Free as in freedom";
    let mut source_data: Vec<u8> = Vec::new();
    for c in source_text.encode_utf16() {
        source_data.extend_from_slice(&c.to_le_bytes());
    }
    source_data.extend_from_slice(&0u16.to_le_bytes()); // null terminator

    // Derive the DES key the same way the reader does:
    // hash /meta (with 2-byte zero prefix) and /DRMStorage/DRMSource, then
    // XOR-fold the 20-byte digest into 8 bytes.
    let mut hasher = MsSha1::new();

    // /meta with 2-byte zero prefix, padded to 64-byte boundary
    let mut meta_padded = vec![0u8; 2];
    meta_padded.extend_from_slice(meta_binary);
    let postpad = 64 - (meta_padded.len() % 64);
    if postpad < 64 {
        meta_padded.resize(meta_padded.len() + postpad, 0);
    }
    hasher.update(&meta_padded);

    // /DRMStorage/DRMSource, padded to 64-byte boundary
    let mut source_padded = source_data.clone();
    let postpad = 64 - (source_padded.len() % 64);
    if postpad < 64 {
        source_padded.resize(source_padded.len() + postpad, 0);
    }
    hasher.update(&source_padded);

    let digest = hasher.finalize();

    let mut des_key = [0u8; 8];
    for (i, &d) in digest.iter().enumerate() {
        des_key[i % 8] ^= d;
    }

    // DRMSealed: encrypt [0x00, key1..key8, padding...] with derived DES key.
    // The sealed data starts with 0x00 followed by the 8-byte book key.
    // For free books, the book key is all zeros (no actual encryption needed).
    let plaintext: [u8; 16] = [0; 16]; // 0x00 prefix + 8 zeros (book key) + 7 zeros
    let cipher = MsDes::new_encrypt(&des_key);
    let sealed_data = cipher.encrypt_ecb(&plaintext);

    // ValidationStream: "MSReader"
    let validation_data = b"MSReader".to_vec();

    (source_data, sealed_data, validation_data)
}

// ---------------------------------------------------------------------------
// AOLL directory chunk builder
// ---------------------------------------------------------------------------

/// Build AOLL directory chunks containing all entries.
///
/// Each chunk has a 48-byte header:
///   "AOLL" (4) + remaining_space(i32) + zeros(8) + chunk_index(i32) + zeros(28)
/// Followed by entry data until the chunk is full.
fn build_aoll_chunks(entries: &[VfsEntry]) -> Vec<Vec<u8>> {
    // Serialize all entries into a flat buffer of encoded entry data.
    let mut encoded_entries: Vec<Vec<u8>> = Vec::new();
    for entry in entries {
        let mut buf = Vec::new();
        let name_bytes = entry.name.as_bytes();
        buf.extend_from_slice(&encode_decint(name_bytes.len() as u64));
        buf.extend_from_slice(name_bytes);
        buf.extend_from_slice(&encode_decint(u64::from(entry.section)));
        buf.extend_from_slice(&encode_decint(entry.data.len() as u64)); // offset placeholder: we use data len here initially
        buf.extend_from_slice(&encode_decint(entry.data.len() as u64));
        encoded_entries.push(buf);
    }

    // Now pack into chunks. We need to calculate actual offsets in section 0.
    // The entries in the directory store: name, section, offset_in_section, size.
    // For section 0, the offset is the cumulative position in the content area.
    let mut offsets: Vec<u64> = Vec::new();
    let mut current_offset: u64 = 0;
    for entry in entries {
        offsets.push(current_offset);
        current_offset += entry.data.len() as u64;
    }

    // Re-encode with correct offsets
    let mut encoded_entries: Vec<Vec<u8>> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let mut buf = Vec::new();
        let name_bytes = entry.name.as_bytes();
        buf.extend_from_slice(&encode_decint(name_bytes.len() as u64));
        buf.extend_from_slice(name_bytes);
        buf.extend_from_slice(&encode_decint(u64::from(entry.section)));
        buf.extend_from_slice(&encode_decint(offsets[i]));
        buf.extend_from_slice(&encode_decint(entry.data.len() as u64));
        encoded_entries.push(buf);
    }

    // Pack into AOLL chunks
    let data_area = CHUNK_SIZE - 48; // 48-byte header
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    let mut current_chunk_data: Vec<u8> = Vec::new();
    let mut chunk_index: i32 = 0;

    for encoded in &encoded_entries {
        if !current_chunk_data.is_empty() && current_chunk_data.len() + encoded.len() > data_area {
            // Flush current chunk
            chunks.push(make_aoll_chunk(chunk_index, &current_chunk_data));
            current_chunk_data.clear();
            chunk_index += 1;
        }
        current_chunk_data.extend_from_slice(encoded);
    }

    // Flush remaining entries
    if !current_chunk_data.is_empty() || chunks.is_empty() {
        chunks.push(make_aoll_chunk(chunk_index, &current_chunk_data));
    }

    chunks
}

/// Build a single AOLL chunk with 48-byte header + entry data + padding.
fn make_aoll_chunk(chunk_index: i32, entry_data: &[u8]) -> Vec<u8> {
    let mut chunk = vec![0u8; CHUNK_SIZE];

    // Magic
    chunk[0..4].copy_from_slice(b"AOLL");

    // remaining_space: offset from end of header to free space.
    // The reader uses this as: remaining = chunk_size - (remaining_raw + 48)
    // So remaining_raw = chunk_size - 48 - used_bytes
    // And remaining = used_bytes
    let used = entry_data.len();
    let remaining_raw = CHUNK_SIZE - 48 - used;
    chunk[4..8].copy_from_slice(&(remaining_raw as i32).to_le_bytes());

    // Bytes 8..16: zeros (already zeroed)
    // Chunk index at offset 16
    chunk[16..20].copy_from_slice(&chunk_index.to_le_bytes());
    // Bytes 20..48: zeros (already zeroed)

    // Entry data at offset 48
    let data_end = (48 + used).min(CHUNK_SIZE);
    chunk[48..data_end].copy_from_slice(&entry_data[..data_end - 48]);

    chunk
}

// ---------------------------------------------------------------------------
// IFCM wrapper
// ---------------------------------------------------------------------------

/// Build an IFCM block wrapping directory chunks.
///
/// Format:
///   "IFCM" (4) + version(u32=1) + chunk_size(i32) + unknown(i32=2) +
///   unknown(i32=0) + unknown(i32=0) + num_chunks(i32) + unknown(i32=0)
///   + chunk_data...
fn build_ifcm(chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();

    // IFCM header (32 bytes)
    out.extend_from_slice(b"IFCM");
    out.extend_from_slice(&1u32.to_le_bytes()); // version
    out.extend_from_slice(&(CHUNK_SIZE as i32).to_le_bytes()); // chunk_size
    out.extend_from_slice(&2i32.to_le_bytes()); // unknown
    out.extend_from_slice(&0i32.to_le_bytes()); // unknown
    out.extend_from_slice(&0i32.to_le_bytes()); // unknown
    out.extend_from_slice(&(chunks.len() as i32).to_le_bytes()); // num_chunks
    out.extend_from_slice(&0i32.to_le_bytes()); // unknown

    for chunk in chunks {
        out.extend_from_slice(chunk);
    }

    out
}

/// Build an IFCM block for the count piece (piece 2).
/// This contains a single AOLL chunk with a count entry.
fn build_count_ifcm(num_entries: usize) -> Vec<u8> {
    // The count piece is a simple IFCM with one AOLL chunk containing
    // a single entry that records the total count.
    let mut entry_data = Vec::new();
    // A single entry: name="", section=0, offset=0, size=num_entries
    entry_data.extend_from_slice(&encode_decint(0)); // name_len = 0
    entry_data.extend_from_slice(&encode_decint(0)); // section
    entry_data.extend_from_slice(&encode_decint(0)); // offset
    entry_data.extend_from_slice(&encode_decint(num_entries as u64)); // size

    let chunk = make_aoll_chunk(0, &entry_data);
    build_ifcm(&[chunk])
}

// ---------------------------------------------------------------------------
// ITOLITLS container builder
// ---------------------------------------------------------------------------

/// Build the complete ITOLITLS container.
fn build_container(entries: &[VfsEntry]) -> Vec<u8> {
    // Build the pieces:
    // Piece 0: File metadata (two u64 values — file length placeholder)
    // Piece 1: IFCM with AOLL directory chunks
    // Piece 2: IFCM with count chunk
    // Piece 3: GUID {0A9007C3-...}
    // Piece 4: GUID {0A9007C4-...}

    let dir_chunks = build_aoll_chunks(entries);
    let piece1 = build_ifcm(&dir_chunks);
    let piece2 = build_count_ifcm(entries.len());
    let piece3 = PIECE3_GUID.to_vec();
    let piece4 = PIECE4_GUID.to_vec();

    // Calculate section 0 content size
    let content_size: usize = entries.iter().map(|e| e.data.len()).sum();

    // Header layout:
    // ITOLITLS (8) + version(4) + header_len(4) + num_pieces(4) + sec_hdr_len(4) + file_guid(16)
    // = 40 bytes of primary header (header_len covers from byte 8 onward = 32 bytes)
    // Then piece table: 5 entries x 16 bytes = 80 bytes
    // Then secondary header: CAOL(48) + ITSF(16) + content_offset(8) + timestamp(4) + locale(4) = 80 bytes
    // ... but the reader expects more in the secondary header. Let me use 232 bytes like the spec says.
    //
    // Actually, looking at the reader code:
    // sec_hdr_offset = hdr_len + num_pieces * 16
    // It reads CAOL block at sec_hdr[off..] where off = i32_le(sec_hdr[4..])
    //
    // Let me keep this simpler. The secondary header needs to contain CAOL and ITSF blocks.

    let num_pieces: usize = 5;
    // hdr_len is the absolute file offset where the piece table starts.
    // Primary header: ITOLITLS(8) + version(4) + hdr_len(4) + num_pieces(4) + sec_hdr_len(4) + guid(16) = 40 bytes
    let primary_hdr_size: usize = 40;
    let piece_table_size: usize = num_pieces * 16;
    let sec_hdr_size: usize = build_secondary_header(CHUNK_SIZE as u32, 0, 0).len();

    let header_total = primary_hdr_size + piece_table_size + sec_hdr_size;

    // Piece data follows the header. Calculate piece offsets.
    let piece0_data = build_piece0(content_size as u64);
    let mut piece_data_offset = header_total;

    let piece0_offset = piece_data_offset;
    piece_data_offset += piece0_data.len();

    let piece1_offset = piece_data_offset;
    piece_data_offset += piece1.len();

    let piece2_offset = piece_data_offset;
    piece_data_offset += piece2.len();

    let piece3_offset = piece_data_offset;
    piece_data_offset += piece3.len();

    let piece4_offset = piece_data_offset;
    piece_data_offset += piece4.len();

    // Content starts after all piece data
    let content_offset = piece_data_offset as u64;

    // Now build the secondary header with the real content_offset
    let sec_hdr = build_secondary_header(CHUNK_SIZE as u32, 0, content_offset);

    // Build piece table
    let piece_table = build_piece_table(&[
        (piece0_offset, piece0_data.len()),
        (piece1_offset, piece1.len()),
        (piece2_offset, piece2.len()),
        (piece3_offset, piece3.len()),
        (piece4_offset, piece4.len()),
    ]);

    // Assemble the file
    let total_size = content_offset as usize + content_size;
    let mut file = Vec::with_capacity(total_size);

    // Primary header
    file.extend_from_slice(MAGIC);
    file.extend_from_slice(&1u32.to_le_bytes()); // version
    file.extend_from_slice(&(primary_hdr_size as i32).to_le_bytes()); // header_len (absolute offset to piece table)
    file.extend_from_slice(&(num_pieces as i32).to_le_bytes());
    file.extend_from_slice(&(sec_hdr.len() as i32).to_le_bytes());
    file.extend_from_slice(&FILE_GUID);

    // Piece table
    file.extend_from_slice(&piece_table);

    // Secondary header
    file.extend_from_slice(&sec_hdr);

    // Piece data
    file.extend_from_slice(&piece0_data);
    file.extend_from_slice(&piece1);
    file.extend_from_slice(&piece2);
    file.extend_from_slice(&piece3);
    file.extend_from_slice(&piece4);

    // Section 0 content: concatenate all entry data in order
    for entry in entries {
        file.extend_from_slice(&entry.data);
    }

    file
}

/// Build piece 0: file metadata. Two u64 values representing total_size.
fn build_piece0(content_size: u64) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&content_size.to_le_bytes());
    out.extend_from_slice(&content_size.to_le_bytes());
    out
}

/// Build the piece table: 5 entries, each 16 bytes (offset:u32, pad:u32, size:u32, pad:u32).
fn build_piece_table(pieces: &[(usize, usize)]) -> Vec<u8> {
    let mut out = Vec::new();
    for &(offset, size) in pieces {
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // pad
        out.extend_from_slice(&(size as i32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // pad
    }
    out
}

/// Build the secondary header containing CAOL and ITSF blocks.
fn build_secondary_header(chunk_size: u32, _unknown: u32, content_offset: u64) -> Vec<u8> {
    let mut out = Vec::new();

    // The secondary header starts with a size field and offset-to-first-block
    // field (observed in real LIT files). The reader does:
    //   off = i32_le(sec_hdr[4..])   // offset to first block within sec_hdr
    // So we put: total_size(u32=80) + offset_to_blocks(i32=8) at the start,
    // then CAOL at offset 8, then ITSF after CAOL.

    let caol_size = 48u32;
    let itsf_size = 48u32;
    let total_hdr = 8 + caol_size + itsf_size; // 104 bytes

    // Header prefix (8 bytes)
    out.extend_from_slice(&total_hdr.to_le_bytes()); // total size
    out.extend_from_slice(&8i32.to_le_bytes()); // offset to first block

    // CAOL block (48 bytes: "CAOL" + 11 u32 fields)
    out.extend_from_slice(b"CAOL"); // offset 8
    out.extend_from_slice(&2u32.to_le_bytes()); // version (CAOL + 4)
    out.extend_from_slice(&48u32.to_le_bytes()); // block size (CAOL + 8)
    out.extend_from_slice(&2u32.to_le_bytes()); // unknown (CAOL + 12)
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown (CAOL + 16)
    out.extend_from_slice(&chunk_size.to_le_bytes()); // entry chunk length (CAOL + 20)
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown (CAOL + 24)
    out.extend_from_slice(&0u32.to_le_bytes()); // entry_unknown (CAOL + 28)
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown (CAOL + 32)
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown (CAOL + 36)
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown (CAOL + 40)
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown (CAOL + 44)

    // ITSF block (48 bytes)
    out.extend_from_slice(b"ITSF"); // offset 56
    out.extend_from_slice(&4u32.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown
    out.extend_from_slice(&(content_offset as u32).to_le_bytes()); // content_offset (offset 16 within ITSF)
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown
    out.extend_from_slice(&0u32.to_le_bytes()); // timestamp
    out.extend_from_slice(&0x0409u32.to_le_bytes()); // locale (en-US)
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown

    out
}

// ---------------------------------------------------------------------------
// OPF generation
// ---------------------------------------------------------------------------

/// Generate OPF XML from book metadata and manifest info.
fn generate_opf(
    book: &Book,
    spine_ids: &[String],
    manifest_entries: &[(String, String, String)], // (id, href, media_type)
) -> String {
    let mut opf = String::new();
    opf.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    opf.push_str("<package unique-identifier=\"bookid\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:oebpackage=\"http://openebook.org/namespaces/oeb-package/1.0/\">\n");
    opf.push_str("  <metadata>\n");
    opf.push_str("    <dc-metadata>\n");

    if let Some(ref title) = book.metadata.title {
        opf.push_str("      <dc:Title>");
        opf.push_str(&escape_xml(title));
        opf.push_str("</dc:Title>\n");
    }
    for author in &book.metadata.authors {
        opf.push_str("      <dc:Creator>");
        opf.push_str(&escape_xml(author));
        opf.push_str("</dc:Creator>\n");
    }
    if let Some(ref lang) = book.metadata.language {
        opf.push_str("      <dc:Language>");
        opf.push_str(&escape_xml(lang));
        opf.push_str("</dc:Language>\n");
    }
    if let Some(ref publisher) = book.metadata.publisher {
        opf.push_str("      <dc:Publisher>");
        opf.push_str(&escape_xml(publisher));
        opf.push_str("</dc:Publisher>\n");
    }
    if let Some(ref desc) = book.metadata.description {
        opf.push_str("      <dc:Description>");
        opf.push_str(&escape_xml(desc));
        opf.push_str("</dc:Description>\n");
    }
    if let Some(ref ident) = book.metadata.identifier {
        opf.push_str("      <dc:Identifier id=\"bookid\">");
        opf.push_str(&escape_xml(ident));
        opf.push_str("</dc:Identifier>\n");
    } else {
        opf.push_str(
            "      <dc:Identifier id=\"bookid\">urn:uuid:eruditio-generated</dc:Identifier>\n",
        );
    }
    for subject in &book.metadata.subjects {
        opf.push_str("      <dc:Subject>");
        opf.push_str(&escape_xml(subject));
        opf.push_str("</dc:Subject>\n");
    }
    if let Some(ref rights) = book.metadata.rights {
        opf.push_str("      <dc:Rights>");
        opf.push_str(&escape_xml(rights));
        opf.push_str("</dc:Rights>\n");
    }

    opf.push_str("    </dc-metadata>\n");
    opf.push_str("  </metadata>\n");

    // Manifest
    opf.push_str("  <manifest>\n");
    for (id, href, media_type) in manifest_entries {
        opf.push_str("    <item id=\"");
        opf.push_str(&escape_xml(id));
        opf.push_str("\" href=\"");
        opf.push_str(&escape_xml(href));
        opf.push_str("\" media-type=\"");
        opf.push_str(&escape_xml(media_type));
        opf.push_str("\" />\n");
    }
    opf.push_str("  </manifest>\n");

    // Spine
    opf.push_str("  <spine>\n");
    for id in spine_ids {
        opf.push_str("    <itemref idref=\"");
        opf.push_str(&escape_xml(id));
        opf.push_str("\" />\n");
    }
    opf.push_str("  </spine>\n");

    opf.push_str("</package>\n");
    opf
}

// ---------------------------------------------------------------------------
// LitWriter
// ---------------------------------------------------------------------------

/// LIT (Microsoft Reader) format writer.
///
/// Produces a valid ITOLITLS container with all content in section 0
/// (Uncompressed). This avoids the need for LZX compression while still
/// producing a file that compliant LIT readers can open.
#[derive(Default)]
pub struct LitWriter;

impl LitWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for LitWriter {
    fn write_book(&self, book: &Book, writer: &mut dyn Write) -> Result<()> {
        let data = write_lit(book)?;
        writer.write_all(&data)?;
        Ok(())
    }
}

/// Produce a complete LIT file from a `Book`.
fn write_lit(book: &Book) -> Result<Vec<u8>> {
    // Collect spine items and their data
    let _resources = book.resources();

    // Build href-to-id mapping for href encoding in binary HTML
    let mut href_to_id: HashMap<String, String> = HashMap::new();
    for item in book.manifest.iter() {
        href_to_id.insert(item.href.clone(), item.id.clone());
    }

    // Binary HTML encoder
    let html_encoder = BinaryEncoder::new_html(href_to_id);
    let opf_encoder = BinaryEncoder::new_opf();

    // Build spine items list for manifest
    let mut spine_manifest: Vec<(String, String, String)> = Vec::new();
    let mut non_spine_manifest: Vec<(String, String, String)> = Vec::new();
    let mut all_manifest: Vec<(String, String, String)> = Vec::new();

    // Virtual filesystem entries
    let mut vfs_entries: Vec<VfsEntry> = Vec::new();

    // Process spine items (chapters)
    let mut spine_ids: Vec<String> = Vec::new();
    for (idx, spine_item) in book.spine.iter().enumerate() {
        let manifest_item = book.manifest.get(&spine_item.manifest_id);
        let item = match manifest_item {
            Some(i) => i,
            None => continue,
        };

        let internal_id = item.id.to_string();
        let content = match item.data.as_text() {
            Some(t) => t.to_string(),
            None => continue,
        };

        // Ensure the content is well-formed XML for the binary encoder.
        // If it doesn't have an <html> wrapper, add one.
        let xml_content = ensure_xhtml(&content);

        let binary_content = html_encoder.encode(&xml_content)?;

        // /data/{id}/content
        vfs_entries.push(VfsEntry {
            name: format!("/data/{internal_id}/content"),
            section: 0,
            data: binary_content,
        });

        // /data/{id}/ahc — anchor hash chunk (4 zero bytes)
        vfs_entries.push(VfsEntry {
            name: format!("/data/{internal_id}/ahc"),
            section: 0,
            data: vec![0u8; 4],
        });

        // /data/{id}/aht — anchor hash table (4 zero bytes)
        vfs_entries.push(VfsEntry {
            name: format!("/data/{internal_id}/aht"),
            section: 0,
            data: vec![0u8; 4],
        });

        let href = item.href.clone();
        let media = item.media_type.clone();

        spine_manifest.push((internal_id.clone(), href.clone(), media.clone()));
        all_manifest.push((internal_id.clone(), href, media));
        spine_ids.push(internal_id);

        let _ = idx; // used only for iteration
    }

    // Process non-spine resources
    for item in book.manifest.iter() {
        // Skip items already processed as spine items
        if spine_ids.contains(&item.id) {
            continue;
        }

        let internal_id = item.id.clone();

        // Store resource data
        if let Some(data) = item.data.as_bytes() {
            vfs_entries.push(VfsEntry {
                name: format!("/data/{internal_id}"),
                section: 0,
                data: data.to_vec(),
            });
        } else if let Some(text) = item.data.as_text() {
            vfs_entries.push(VfsEntry {
                name: format!("/data/{internal_id}"),
                section: 0,
                data: text.as_bytes().to_vec(),
            });
        } else {
            continue;
        }

        let href = item.href.clone();
        let media = item.media_type.clone();

        non_spine_manifest.push((internal_id.clone(), href.clone(), media.clone()));
        all_manifest.push((internal_id, href, media));
    }

    // Build manifest entry
    let manifest_data = build_manifest(&spine_manifest, &non_spine_manifest);
    vfs_entries.push(VfsEntry {
        name: "/manifest".to_string(),
        section: 0,
        data: manifest_data,
    });

    // Build OPF and binary-encode it as /meta
    let opf_xml = generate_opf(book, &spine_ids, &all_manifest);
    let meta_binary = opf_encoder.encode(&opf_xml)?;
    vfs_entries.push(VfsEntry {
        name: "/meta".to_string(),
        section: 0,
        data: meta_binary.clone(),
    });

    // /Version: 4 bytes = pack('<HH', 8, 1) = version 8.1
    let mut version_data = Vec::new();
    version_data.extend_from_slice(&8u16.to_le_bytes());
    version_data.extend_from_slice(&1u16.to_le_bytes());
    vfs_entries.push(VfsEntry {
        name: "/Version".to_string(),
        section: 0,
        data: version_data,
    });

    // ::DataSpace/NameList — section name list with only "Uncompressed"
    let namelist = build_namelist(&["Uncompressed"]);
    vfs_entries.push(VfsEntry {
        name: "::DataSpace/NameList".to_string(),
        section: 0,
        data: namelist,
    });

    // DRM storage entries (free book)
    let (drm_source, drm_sealed, validation) = build_free_drm(&meta_binary);
    vfs_entries.push(VfsEntry {
        name: "/DRMStorage/DRMSource".to_string(),
        section: 0,
        data: drm_source,
    });
    vfs_entries.push(VfsEntry {
        name: "/DRMStorage/DRMSealed".to_string(),
        section: 0,
        data: drm_sealed,
    });
    vfs_entries.push(VfsEntry {
        name: "/DRMStorage/ValidationStream".to_string(),
        section: 0,
        data: validation,
    });

    // Build the complete container
    let container = build_container(&vfs_entries);
    Ok(container)
}

/// Ensure content is wrapped in an XHTML structure for the XML parser.
fn ensure_xhtml(content: &str) -> String {
    let trimmed = content.trim();

    // If it already has an <html> tag, assume it's complete XHTML
    if trimmed.starts_with("<?xml") || trimmed.starts_with("<html") || trimmed.starts_with("<HTML")
    {
        return trimmed.to_string();
    }

    // If it starts with a <body> or <head> tag, wrap in <html>
    if trimmed.starts_with("<body") || trimmed.starts_with("<head") {
        return format!("<html>{trimmed}</html>");
    }

    // Otherwise, wrap in a minimal XHTML structure
    format!("<html><body>{trimmed}</body></html>")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::LitReader;
    use super::*;
    use crate::domain::{Book, Chapter, FormatReader};

    #[test]
    fn encode_decint_zero() {
        assert_eq!(encode_decint(0), vec![0]);
    }

    #[test]
    fn encode_decint_small() {
        assert_eq!(encode_decint(1), vec![1]);
        assert_eq!(encode_decint(127), vec![127]);
    }

    #[test]
    fn encode_decint_two_bytes() {
        // 128 = 1 << 7 = 0x81 0x00
        assert_eq!(encode_decint(128), vec![0x81, 0x00]);
    }

    #[test]
    fn encode_decint_roundtrip() {
        use crate::formats::common::itss;
        for val in [0, 1, 127, 128, 255, 1000, 16384, 100_000] {
            let encoded = encode_decint(val);
            let (decoded, consumed) = itss::encint(&encoded).unwrap();
            assert_eq!(decoded, val, "roundtrip failed for {val}");
            assert_eq!(consumed, encoded.len());
        }
    }

    #[test]
    fn encode_utf8_ordinal_ascii() {
        assert_eq!(encode_utf8_ordinal(65), b"A".to_vec());
    }

    #[test]
    fn encode_utf8_ordinal_zero() {
        assert_eq!(encode_utf8_ordinal(0), vec![0]);
    }

    #[test]
    fn build_namelist_single() {
        let data = build_namelist(&["Uncompressed"]);
        // Should start with u16(0) + u16(1) = 4 bytes
        assert_eq!(data[0..2], [0, 0]);
        assert_eq!(data[2..4], [1, 0]); // num_sections = 1
    }

    #[test]
    fn binary_encoder_simple_text() {
        let encoder = BinaryEncoder::new_html(HashMap::new());
        let result = encoder.encode("Hello").unwrap();
        assert_eq!(result, b"Hello");
    }

    #[test]
    fn binary_encoder_simple_tag() {
        let encoder = BinaryEncoder::new_html(HashMap::new());
        // <p>Test</p> should produce binary tokens
        let result = encoder.encode("<p>Test</p>").unwrap();
        // Should contain the text "Test"
        assert!(result.windows(4).any(|w| w == b"Test"));
        // Should start with 0x00 (tag start marker)
        assert_eq!(result[0], 0x00);
    }

    #[test]
    fn escape_xml_special_chars() {
        assert_eq!(escape_xml("a & b"), "a &amp; b");
        assert_eq!(escape_xml("<tag>"), "&lt;tag&gt;");
        assert_eq!(escape_xml("\"quoted\""), "&quot;quoted&quot;");
    }

    #[test]
    fn ensure_xhtml_wraps_bare_content() {
        let result = ensure_xhtml("<p>Hello</p>");
        assert!(result.contains("<html>"));
        assert!(result.contains("<body>"));
        assert!(result.contains("<p>Hello</p>"));
    }

    #[test]
    fn ensure_xhtml_preserves_complete() {
        let input = "<html><body><p>Hello</p></body></html>";
        let result = ensure_xhtml(input);
        assert_eq!(result, input);
    }

    #[test]
    fn lit_writer_produces_valid_header() {
        let mut book = Book::new();
        book.metadata.title = Some("Test Book".into());
        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello, world!</p>".into(),
            id: Some("ch1".into()),
        });

        let data = write_lit(&book).unwrap();

        // Check ITOLITLS header
        assert_eq!(&data[0..8], b"ITOLITLS");
        // Version = 1
        assert_eq!(
            u32::from_le_bytes([data[8], data[9], data[10], data[11]]),
            1
        );

        // Verify the reader can locate the secondary header
        let hdr_len = i32::from_le_bytes([data[12], data[13], data[14], data[15]]) as usize;
        let num_pieces = i32::from_le_bytes([data[16], data[17], data[18], data[19]]) as usize;
        let sec_hdr_len = i32::from_le_bytes([data[20], data[21], data[22], data[23]]) as usize;

        assert_eq!(
            hdr_len, 40,
            "hdr_len should be 40 (absolute offset to piece table)"
        );
        assert_eq!(num_pieces, 5, "should have 5 pieces");

        let sec_hdr_offset = hdr_len + num_pieces * 16;
        assert!(
            sec_hdr_offset + sec_hdr_len <= data.len(),
            "secondary header ({sec_hdr_offset}..{}) should fit in file (len={})",
            sec_hdr_offset + sec_hdr_len,
            data.len()
        );

        let sec_hdr = &data[sec_hdr_offset..sec_hdr_offset + sec_hdr_len];
        assert!(sec_hdr.len() >= 8, "secondary header too short");
        let off = i32::from_le_bytes([sec_hdr[4], sec_hdr[5], sec_hdr[6], sec_hdr[7]]) as usize;
        assert!(
            off + 4 <= sec_hdr.len(),
            "CAOL offset out of bounds: off={off}, len={}",
            sec_hdr.len()
        );
        assert_eq!(
            &sec_hdr[off..off + 4],
            b"CAOL",
            "CAOL not at expected offset {off}"
        );

        // After CAOL (48 bytes), ITSF should follow
        let itsf_off = off + 48;
        assert!(
            itsf_off + 4 <= sec_hdr.len(),
            "ITSF offset out of bounds: off={itsf_off}, len={}",
            sec_hdr.len()
        );
        assert_eq!(
            &sec_hdr[itsf_off..itsf_off + 4],
            b"ITSF",
            "ITSF not at expected offset {itsf_off}"
        );
    }

    #[test]
    fn lit_roundtrip_basic() {
        let mut book = Book::new();
        book.metadata.title = Some("Round Trip Test".into());
        book.metadata.authors.push("Test Author".into());
        book.metadata.language = Some("en".into());

        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: "<html><body><p>Hello from chapter one.</p></body></html>".into(),
            id: Some("ch1".into()),
        });

        book.add_chapter(Chapter {
            title: Some("Chapter 2".into()),
            content: "<html><body><p>Goodbye from chapter two.</p></body></html>".into(),
            id: Some("ch2".into()),
        });

        // Write
        let data = write_lit(&book).unwrap();

        // Read back
        let reader = LitReader::new();
        let mut cursor = std::io::Cursor::new(data);
        let read_book = reader.read_book(&mut cursor).unwrap();

        // Verify metadata
        assert_eq!(read_book.metadata.title.as_deref(), Some("Round Trip Test"));
        assert_eq!(read_book.metadata.authors, vec!["Test Author"]);
        assert_eq!(read_book.metadata.language.as_deref(), Some("en"));

        // Verify chapters exist
        let chapters = read_book.chapters();
        assert_eq!(chapters.len(), 2);

        // Verify chapter content contains expected text
        assert!(
            chapters[0].content.contains("Hello from chapter one"),
            "Chapter 1 content mismatch: {}",
            &chapters[0].content[..chapters[0].content.len().min(200)]
        );
        assert!(
            chapters[1].content.contains("Goodbye from chapter two"),
            "Chapter 2 content mismatch: {}",
            &chapters[1].content[..chapters[1].content.len().min(200)]
        );
    }

    #[test]
    fn lit_roundtrip_with_resource() {
        let mut book = Book::new();
        book.metadata.title = Some("Resource Test".into());

        book.add_chapter(Chapter {
            title: Some("Ch 1".into()),
            content: "<html><body><p>Text content</p></body></html>".into(),
            id: Some("ch1".into()),
        });

        book.add_resource(
            "img1",
            "images/test.png",
            vec![0x89, 0x50, 0x4E, 0x47],
            "image/png",
        );

        let data = write_lit(&book).unwrap();
        let reader = LitReader::new();
        let mut cursor = std::io::Cursor::new(data);
        let read_book = reader.read_book(&mut cursor).unwrap();

        assert_eq!(read_book.metadata.title.as_deref(), Some("Resource Test"));
        let chapters = read_book.chapters();
        assert_eq!(chapters.len(), 1);
    }
}
