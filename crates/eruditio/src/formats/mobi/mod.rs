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
use crate::formats::common::MAX_INPUT_SIZE;
use crate::formats::common::compression::huffcdic::HuffCdicReader;
use crate::formats::common::compression::palmdoc;
use crate::formats::common::palm_db::{PdbFile, read_u32_be};
use crate::formats::common::text_utils::{self, decode_cp1252, strip_tags};
use std::borrow::Cow;
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
        if let Ok(data) = pdb.record_data(i)
            && data.len() >= 4
            && &data[..4] == b"FDST"
        {
            return Some(i);
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

/// Resolves kindle:embed, kindle:flow, and kindle:pos:fid references in HTML content.
/// Returns the HTML with references replaced by actual resource paths.
///
/// - `image_paths`: indexed by 0-based image record number
/// - `flow_paths`: indexed by flow number (flow 0 = None since it's the main content)
/// - `chapter_count`: total number of content parts (for kindle:pos:fid resolution)
fn resolve_kindle_references<'a>(
    html: &'a str,
    image_paths: &[String],
    flow_paths: &[Option<String>],
    chapter_count: usize,
) -> Cow<'a, str> {
    if !html.contains("kindle:") {
        return Cow::Borrowed(html);
    }

    let mut result = String::with_capacity(html.len());
    let mut remaining = html;

    while let Some(pos) = remaining.find("kindle:") {
        result.push_str(&remaining[..pos]);
        let after_kindle = &remaining[pos + 7..]; // skip "kindle:"

        if let Some(replacement) = try_resolve_embed(after_kindle, image_paths) {
            result.push_str(replacement.0);
            remaining = &remaining[pos + 7 + replacement.1..];
        } else if let Some(replacement) = try_resolve_flow(after_kindle, flow_paths) {
            result.push_str(replacement.0);
            remaining = &remaining[pos + 7 + replacement.1..];
        } else if let Some(replacement) = try_resolve_pos_fid(after_kindle, chapter_count) {
            result.push_str(&replacement.0);
            remaining = &remaining[pos + 7 + replacement.1..];
        } else {
            // Unresolvable reference; keep the "kindle:" prefix and advance past it.
            result.push_str("kindle:");
            remaining = after_kindle;
        }
    }

    result.push_str(remaining);
    Cow::Owned(result)
}

/// Tries to resolve a kindle:embed:XXXX reference.
/// Returns (replacement_string, bytes_consumed_after_"kindle:") or None.
///
/// kindle:embed indices are 1-based: kindle:embed:0001 refers to the first image.
fn try_resolve_embed<'a>(
    after_kindle: &str,
    image_paths: &'a [String],
) -> Option<(&'a str, usize)> {
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
    Some((path.as_str(), total_consumed))
}

/// Tries to resolve a kindle:flow:XXXX reference.
/// Flow indices use the same Kindle base-32 encoding as embed references
/// (digits 0-9 plus letters A-V).
/// Returns (replacement_string, bytes_consumed_after_"kindle:") or None.
fn try_resolve_flow<'a>(
    after_kindle: &str,
    flow_paths: &'a [Option<String>],
) -> Option<(&'a str, usize)> {
    let rest = after_kindle.strip_prefix("flow:")?;
    let consumed_prefix = 5; // "flow:"

    // Extract the base-32 encoded index (digits 0-9 and letters A-V).
    let code_end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() || c > 'V' && c <= 'Z' || c > 'v' && c <= 'z')
        .unwrap_or(rest.len());
    if code_end == 0 {
        return None;
    }
    let code = &rest[..code_end];
    let index = decode_kindle_base32(code)?;

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
    Some((path.as_str(), total_consumed))
}

/// Tries to resolve a kindle:pos:fid:XXXX:off:YYYYYY reference.
///
/// These are KF8 internal cross-references encoding a fragment ID (which part
/// of the book) and a byte offset within that part.
///
/// Returns (replacement_string, bytes_consumed_after_"kindle:") or None.
/// Parses a `pos:fid:XXXX:off:YYYY` reference from the string following `"kindle:"`.
///
/// Returns `(fid, offset, bytes_consumed_after_"kindle:")` or `None` if malformed.
fn parse_pos_fid(after_kindle: &str) -> Option<(usize, usize, usize)> {
    let rest = after_kindle.strip_prefix("pos:fid:")?;
    let consumed_prefix = 8; // "pos:fid:"

    // Extract the fid code (base-32 encoded, terminated by ':').
    let fid_end = rest.find(':')?;
    if fid_end == 0 {
        return None;
    }
    let fid_code = &rest[..fid_end];
    let fid = decode_kindle_base32(fid_code)?;

    // Skip ":off:".
    let after_fid = &rest[fid_end..];
    let off_rest = after_fid.strip_prefix(":off:")?;
    let off_prefix_len = 5; // ":off:"

    // Extract the offset code (base-32 encoded).
    let off_end = off_rest
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(off_rest.len());
    if off_end == 0 {
        return None;
    }
    let off_code = &off_rest[..off_end];
    let offset = decode_kindle_base32(off_code)?;

    let total_consumed = consumed_prefix + fid_end + off_prefix_len + off_end;
    Some((fid, offset, total_consumed))
}

fn try_resolve_pos_fid(after_kindle: &str, chapter_count: usize) -> Option<(String, usize)> {
    let (fid, _offset, total_consumed) = parse_pos_fid(after_kindle)?;

    if chapter_count == 0 || fid >= chapter_count {
        return None;
    }
    let replacement = format!("mobi_ch_{}.xhtml", fid);
    Some((replacement, total_consumed))
}

// --- INDX record parsing and PosFidResolver for full kindle:pos:fid resolution ---

/// Parses the INDX record header and returns key fields.
/// Returns (count, idxt_offset) or None if invalid.
fn parse_indx_header_fields(data: &[u8]) -> Option<(usize, usize)> {
    if data.len() < 4 || &data[..4] != b"INDX" {
        return None;
    }
    // The header contains 44 u32 fields after the magic.
    // Key offsets (as u32 field indices): 4=start (IDXT position), 5=count
    if data.len() < 4 + 6 * 4 {
        return None;
    }
    let count = read_u32_be(data, 4 + 5 * 4) as usize; // field 5: count
    let idxt_start = read_u32_be(data, 4 + 4 * 4) as usize; // field 4: start (IDXT position)
    Some((count, idxt_start))
}

/// Finds the TAGX section in an INDX header record and returns the
/// control_byte_count. Handles the case where the tagx offset field is 0
/// by scanning for the TAGX signature.
fn find_tagx_control_byte_count(data: &[u8]) -> Option<usize> {
    // Try the tagx offset from header field 43.
    let header_tagx = if data.len() >= 4 + 44 * 4 {
        read_u32_be(data, 4 + 43 * 4) as usize
    } else {
        0
    };

    let mut tagx_offset = header_tagx;
    if tagx_offset + 12 > data.len()
        || data.len() < tagx_offset + 4
        || &data[tagx_offset..tagx_offset + 4] != b"TAGX"
    {
        // Fallback: scan for TAGX after header fields.
        let scan_start = if data.len() >= 4 + 44 * 4 {
            4 + 44 * 4
        } else {
            4
        };
        let sub = &data[scan_start..];
        tagx_offset = sub
            .windows(4)
            .position(|w| w == b"TAGX")
            .map(|pos| scan_start + pos)?;
    }

    if tagx_offset + 12 > data.len() || &data[tagx_offset..tagx_offset + 4] != b"TAGX" {
        return None;
    }

    let control_byte_count = read_u32_be(data, tagx_offset + 8) as usize;
    Some(control_byte_count)
}

/// Extracts the text keys (insert positions) from a KF8 div/fragment INDX index.
///
/// The div table entries have text keys that are zero-padded numeric strings
/// representing byte positions (insert positions) in the raw text stream.
/// Each entry corresponds to a content fragment; the `fid` value in
/// `kindle:pos:fid:XXXX:off:YYYY` is an index into this table.
fn parse_div_insert_positions(pdb: &PdbFile, fragment_index: usize) -> Option<Vec<usize>> {
    let header_data = pdb.record_data(fragment_index).ok()?;
    let (indx_count, _) = parse_indx_header_fields(header_data)?;

    // Validate that the INDX record contains a TAGX section (well-formed structure).
    let _control_byte_count = find_tagx_control_byte_count(header_data)?;

    let mut insert_positions = Vec::new();

    for rec_idx in 1..=indx_count {
        let abs_idx = fragment_index + rec_idx;
        let rec_data = match pdb.record_data(abs_idx) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let (entry_count, idxt_pos) = match parse_indx_header_fields(rec_data) {
            Some(v) => v,
            None => continue,
        };

        // Verify IDXT signature.
        if idxt_pos + 4 > rec_data.len() || &rec_data[idxt_pos..idxt_pos + 4] != b"IDXT" {
            continue;
        }

        // Read IDXT entry offsets.
        let mut idx_positions = Vec::with_capacity(entry_count + 1);
        for j in 0..entry_count {
            let pos_offset = idxt_pos + 4 + j * 2;
            if pos_offset + 2 > rec_data.len() {
                break;
            }
            let pos = ((rec_data[pos_offset] as usize) << 8) | (rec_data[pos_offset + 1] as usize);
            idx_positions.push(pos);
        }
        idx_positions.push(idxt_pos); // sentinel: last entry ends at IDXT

        // Extract text keys from each entry.
        for j in 0..entry_count {
            if j + 1 >= idx_positions.len() {
                break;
            }
            let start = idx_positions[j];
            let end = idx_positions[j + 1];
            if start >= end || start >= rec_data.len() {
                insert_positions.push(0); // placeholder for invalid entries
                continue;
            }
            let entry = &rec_data[start..end.min(rec_data.len())];
            if entry.is_empty() {
                insert_positions.push(0);
                continue;
            }

            // First byte = text key length, followed by key bytes.
            let key_len = entry[0] as usize;
            if key_len == 0 || key_len + 1 > entry.len() {
                insert_positions.push(0);
                continue;
            }
            let key_bytes = &entry[1..1 + key_len];

            // The text key is a zero-padded numeric string (the insert position).
            match std::str::from_utf8(key_bytes)
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
            {
                Some(pos) => insert_positions.push(pos),
                None => insert_positions.push(0),
            }
        }
    }

    if insert_positions.is_empty() {
        None
    } else {
        Some(insert_positions)
    }
}

/// Splits raw bytes at `<?xml` boundaries and returns byte offset ranges
/// for each resulting part. Falls back to `<html` boundaries if no multiple
/// `<?xml` declarations are found.
fn split_bytes_at_xml_boundaries(data: &[u8]) -> Vec<(usize, usize)> {
    let mut positions = find_all_tags(data, b"<?xml");

    if positions.len() <= 1 {
        let html_positions = find_all_tags(data, b"<html");
        if html_positions.len() > 1 {
            positions = html_positions;
        }
    }

    if positions.is_empty() {
        return vec![(0, data.len())];
    }

    let mut ranges = Vec::with_capacity(positions.len());
    for i in 0..positions.len() {
        let start = positions[i];
        let end = if i + 1 < positions.len() {
            positions[i + 1]
        } else {
            data.len()
        };
        ranges.push((start, end));
    }
    ranges
}

/// Resolver for kindle:pos:fid references using the KF8 div table.
///
/// Maps (fid, offset) pairs to (chapter_filename, anchor_id) by:
/// 1. Looking up the insert position from the div table entry at index `fid`
/// 2. Adding the offset to get an absolute byte position in the raw text
/// 3. Finding which chapter contains that position
/// 4. Searching backwards for the nearest id/name anchor
///
/// Borrows the decompressed text data to avoid copying each chapter's bytes.
struct PosFidResolver<'a> {
    /// Insert positions from the div table, indexed by div entry number.
    insert_positions: Vec<usize>,
    /// Absolute byte offset of flow 0 within the full decompressed text.
    flow0_start: usize,
    /// Byte ranges (start, end) for each chapter, relative to `text_data`.
    chapter_ranges: &'a [(usize, usize)],
    /// Borrowed reference to the main HTML bytes (flow 0 of the FDST).
    text_data: &'a [u8],
}

impl PosFidResolver<'_> {
    fn resolve(&self, fid: usize, offset: usize) -> Option<(String, Option<String>)> {
        let insert_pos = *self.insert_positions.get(fid)?;
        let abs_pos = insert_pos.checked_add(offset)?;
        let rel_pos = abs_pos.checked_sub(self.flow0_start)?;

        // Find which chapter contains this relative position.
        let chapter_idx = self
            .chapter_ranges
            .iter()
            .position(|(start, end)| rel_pos >= *start && rel_pos < *end)?;

        let (ch_start, ch_end) = self.chapter_ranges[chapter_idx];
        let chapter_data = self.text_data.get(ch_start..ch_end)?;

        // Compute position within the chapter's byte data.
        let pos_in_chapter = rel_pos.saturating_sub(ch_start).min(chapter_data.len());

        // Search backwards for the nearest id= or name= anchor.
        let anchor = find_nearest_anchor(chapter_data, pos_in_chapter);

        let filename = format!("mobi_ch_{}.xhtml", chapter_idx);
        Some((filename, anchor))
    }
}

/// Searches backwards from a byte position in XHTML content to find the
/// nearest element with an `id` or `name` attribute. Mirrors Calibre's
/// `get_id_tag()` method.
fn find_nearest_anchor(data: &[u8], pos: usize) -> Option<String> {
    let pos = pos.min(data.len());

    // If pos is inside a tag, extend to end of that tag.
    let search_end = {
        let next_gt = data[pos..].iter().position(|&b| b == b'>').map(|i| pos + i);
        let next_lt = data[pos..].iter().position(|&b| b == b'<').map(|i| pos + i);
        match (next_gt, next_lt) {
            (Some(gt), Some(lt)) if gt < lt => gt + 1,
            (Some(gt), None) => gt + 1,
            _ => pos,
        }
    };

    let block = &data[..search_end];

    // Iterate over tags in reverse order.
    let mut end = block.len();
    while let Some(pgt) = block[..end].iter().rposition(|&b| b == b'>') {
        let plt = match block[..pgt].iter().rposition(|&b| b == b'<') {
            Some(p) => p,
            None => break,
        };
        let tag = &block[plt..pgt + 1];

        // Check for id="..." (on any tag) or name="..." (only on <a> tags).
        if let Some(val) = extract_attr_value(tag, b"id")
            && !val.is_empty()
        {
            return Some(val.to_string());
        }
        // Only check name= on <a> tags to avoid matching <meta name="..."> etc.
        if tag.len() >= 2
            && tag[0] == b'<'
            && tag[1].eq_ignore_ascii_case(&b'a')
            && (tag.len() <= 2 || !tag[2].is_ascii_alphanumeric())
            && let Some(val) = extract_attr_value(tag, b"name")
            && !val.is_empty()
        {
            return Some(val.to_string());
        }

        end = plt;
    }

    None
}

/// Extracts the value of a named attribute from an HTML/XML tag byte slice.
/// Case-insensitive attribute name matching, supports both single and double quotes.
///
/// Zero-allocation: uses inline case-insensitive comparison instead of
/// lowercasing the entire tag into a temporary `Vec<u8>`.
fn extract_attr_value<'t>(tag: &'t [u8], attr_name: &[u8]) -> Option<&'t str> {
    // Build the search pattern ` attr=` on the stack (attr names are short).
    let pattern_len = 1 + attr_name.len() + 1; // space + name + '='
    debug_assert!(
        pattern_len <= 64,
        "attribute name too long for stack buffer"
    );

    // Scan through the tag looking for ` attr=` case-insensitively.
    // We need at least pattern_len bytes to match.
    let attr_pos = tag.windows(pattern_len).position(|w| {
        w[0] == b' '
            && w[1..1 + attr_name.len()].eq_ignore_ascii_case(attr_name)
            && w[1 + attr_name.len()] == b'='
    })?;
    let value_start = attr_pos + pattern_len;

    if value_start >= tag.len() {
        return None;
    }

    // Skip whitespace after '='.
    let rest = &tag[value_start..];
    let trimmed = rest
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .map(|i| &rest[i..])
        .unwrap_or(rest);

    if trimmed.is_empty() {
        return None;
    }

    let quote = trimmed[0];
    if quote != b'"' && quote != b'\'' {
        return None;
    }

    let value_bytes = &trimmed[1..];
    let end = value_bytes.iter().position(|&b| b == quote)?;
    std::str::from_utf8(&value_bytes[..end]).ok()
}

/// Resolves remaining `kindle:pos:fid` references in HTML content using
/// the PosFidResolver. This is called as a second pass after the basic
/// `resolve_kindle_references` to handle cross-file references that the
/// simple fid-to-chapter mapping couldn't resolve.
fn resolve_remaining_pos_fid<'a>(html: &'a str, resolver: &PosFidResolver<'_>) -> Cow<'a, str> {
    if !html.contains("kindle:pos:fid:") {
        return Cow::Borrowed(html);
    }

    let mut result = String::with_capacity(html.len());
    let mut remaining = html;

    while let Some(pos) = remaining.find("kindle:pos:fid:") {
        result.push_str(&remaining[..pos]);
        let after_kindle = &remaining[pos + 7..]; // skip "kindle:"

        if let Some((fid, offset, consumed)) = parse_pos_fid(after_kindle) {
            let total_consumed = 7 + consumed; // 7 for "kindle:"

            // Try to resolve using the full resolver.
            if let Some((filename, anchor)) = resolver.resolve(fid, offset) {
                let replacement = if let Some(anchor_id) = anchor {
                    format!("{}#{}", filename, anchor_id)
                } else {
                    filename
                };
                result.push_str(&replacement);
            } else {
                // Could not resolve: keep the original reference.
                result.push_str(&remaining[pos..pos + total_consumed]);
            }
            remaining = &remaining[pos + total_consumed..];
        } else {
            // Malformed pos:fid reference; keep "kindle:" and advance.
            result.push_str("kindle:");
            remaining = after_kindle;
        }
    }

    result.push_str(remaining);
    Cow::Owned(result)
}

/// Detects the content type and file extension of a KF8 flow resource.
fn detect_flow_type(data: &[u8]) -> (&'static str, &'static str) {
    let trimmed = trim_start_whitespace(data);
    if trimmed.starts_with(b"<svg") || trimmed.starts_with(b"<SVG") {
        ("svg", "image/svg+xml")
    } else if trimmed.starts_with(b"<?xml") {
        // XML content: check if it contains an SVG element, otherwise treat as CSS
        // (KF8 sometimes wraps CSS in CDATA within XML).
        if data.windows(4).any(|w| w.eq_ignore_ascii_case(b"<svg")) {
            ("svg", "image/svg+xml")
        } else {
            ("css", "text/css")
        }
    } else {
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

/// Finds all occurrences of an HTML tag pattern, case-insensitively.
///
/// Unlike `find_ci` (which only needs the first match), this must return
/// ALL positions — including mixed-case variants. A document with both
/// `<html` and `<HTML` must find both. The case-insensitive search is
/// always used to ensure completeness.
fn find_all_tags(haystack: &[u8], lowercase_pattern: &[u8]) -> Vec<usize> {
    text_utils::find_all_case_insensitive(haystack, lowercase_pattern)
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
        // Pre-allocate 4 MB to reduce Vec doubling during read_to_end.
        // MOBI files are typically 1-50 MB; starting at 4 MB cuts geometric
        // growth from ~14 doublings to ~3 for a 25 MB file, eliminating
        // ~40 MB of transient allocations.
        let mut buffer = Vec::with_capacity(4 << 20);
        (&mut *reader)
            .take(MAX_INPUT_SIZE)
            .read_to_end(&mut buffer)?;

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

        // Pre-compute all PDB-dependent data so we can drop the raw buffer early.
        let first_image = mobi_header
            .as_ref()
            .map(|h| h.first_image_index as usize)
            .filter(|&idx| idx != NULL_INDEX as usize)
            .unwrap_or(pdb.record_count());

        let fdst_entries = if is_kf8 {
            find_fdst_record(&pdb, first_image).and_then(|idx| parse_fdst(&pdb, idx))
        } else {
            None
        };

        let div_insert_positions = if is_kf8 {
            mobi_header
                .as_ref()
                .and_then(|h| h.fragment_index)
                .filter(|&idx| idx != NULL_INDEX)
                .and_then(|frag_idx| parse_div_insert_positions(&pdb, frag_idx as usize))
        } else {
            None
        };

        // All data has been extracted from the PDB; drop the raw 25 MB buffer.
        drop(pdb);

        if is_kf8 {
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
                    if entry.start <= text.len()
                        && entry.end <= text.len()
                        && entry.start < entry.end
                    {
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

            // Compute byte offset ranges for each chapter in the raw bytes
            // (before string decoding). These are needed for proper pos:fid
            // resolution since kindle:pos:fid positions are byte offsets.
            let flow0_start = fdst_entries
                .as_ref()
                .and_then(|e| e.first())
                .map(|e| e.start)
                .unwrap_or(0);
            let chapter_byte_ranges = split_bytes_at_xml_boundaries(main_html_bytes);

            // Build the PosFidResolver using the div table from the fragment index.
            let pos_fid_resolver = div_insert_positions.map(|insert_positions| PosFidResolver {
                insert_positions,
                flow0_start,
                chapter_ranges: &chapter_byte_ranges,
                text_data: main_html_bytes,
            });

            // Split KF8 content on XHTML document boundaries first,
            // so we know how many chapters exist for kindle:pos:fid resolution.
            let raw_chapters = split_kf8_content(&html_string);
            let chapter_count = raw_chapters.len();

            // Resolve kindle: references (embed, flow, pos:fid) in each chapter.
            // When a PosFidResolver is available, skip the naive first-pass
            // fid→chapter mapping (which lacks anchor precision) so ALL
            // kindle:pos:fid references go through the anchor-aware second pass.
            let effective_chapter_count = if pos_fid_resolver.is_some() {
                0
            } else {
                chapter_count
            };
            for (i, ch) in raw_chapters.into_iter().enumerate() {
                let resolved = resolve_kindle_references(
                    &ch.content,
                    &image_paths,
                    &flow_paths,
                    effective_chapter_count,
                );

                // Second pass: use the full PosFidResolver for any remaining
                // kindle:pos:fid references (cross-file TOC links, etc.).
                let final_content = if let Some(ref resolver) = pos_fid_resolver
                    && resolved.contains("kindle:pos:fid:")
                {
                    resolve_remaining_pos_fid(&resolved, resolver).into_owned()
                } else {
                    resolved.into_owned()
                };

                book.add_chapter(Chapter {
                    title: ch.title,
                    content: final_content,
                    id: Some(format!("mobi_ch_{}", i)),
                });
            }
        } else {
            // Non-KF8: original behavior.
            let content = if mobi_header.as_ref().is_some_and(|h| h.is_utf8()) {
                crate::formats::common::text_utils::bytes_to_string(&text)
            } else {
                decode_cp1252(&text)
            };

            // Split into chapters by pagebreaks or treat as single chapter.
            let chapters = split_mobi_content(&content);
            for (i, ch) in chapters.into_iter().enumerate() {
                book.add_chapter(Chapter {
                    title: ch.title,
                    content: ch.content.into_owned(),
                    id: Some(format!("mobi_ch_{}", i)),
                });
            }
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
                reader.unpack_into(record_data, &mut text).map_err(|e| {
                    EruditioError::Compression(format!("HUFF/CDIC decompression failed: {}", e))
                })?;
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

/// Splits KF8 HTML content into chapters by detecting concatenated XHTML documents.
///
/// KF8 (AZW3) files store all content as a single byte stream in flow 0, but
/// this stream actually contains multiple complete XHTML documents concatenated
/// together. Each document has its own `<?xml` declaration, `<html>`, `<head>`,
/// and `<body>` elements.
///
/// In the raw KF8 byte stream, each XHTML "document" consists of a skeleton
/// (the `<html>`/`<head>`/`<body>` wrapper with an empty body) followed by
/// content fragments that belong inside the `<body>`. When we split at `<?xml`
/// boundaries, the content fragments end up *after* the closing `</html>` tag.
/// This function detects those boundaries, splits, and then reassembles each
/// part so the content is correctly placed inside `<body>`.
///
/// If no multiple documents are detected, it falls back to `split_mobi_content()`
/// (pagebreak-based splitting).
fn split_kf8_content<'a>(html: &'a str) -> Vec<SimpleChapter<'a>> {
    let parts = split_on_xhtml_boundaries(html);

    if parts.len() <= 1 {
        return split_mobi_content(html);
    }

    let mut chapters = Vec::with_capacity(parts.len());
    let mut untitled_counter = 0usize;
    for part in parts.iter() {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }

        let fixed = reassemble_kf8_xhtml(trimmed);

        let title = extract_first_heading(&fixed)
            .or_else(|| extract_fallback_title(&fixed))
            .map(|t| sanitize_toc_label(&t).into_owned())
            .or_else(|| {
                untitled_counter += 1;
                Some(format!("Untitled Section {}", untitled_counter))
            });

        chapters.push(SimpleChapter {
            title,
            content: fixed,
        });
    }

    if chapters.is_empty() {
        chapters.push(SimpleChapter {
            title: None,
            content: Cow::Borrowed(html),
        });
    }

    chapters
}

/// Reassembles a KF8 XHTML part by moving content from after `</html>` into
/// the `<body>` element.
///
/// In the raw KF8 byte stream, each skeleton provides the document wrapper
/// (`<?xml>`, `<html>`, `<head>`, `<body>`) with an empty body, and the actual
/// chapter content (headings, paragraphs, images) follows after the `</html>`
/// closing tag. This function fixes that by:
///
/// 1. Extracting the trailing content (everything after `</html>`)
/// 2. Finding the `</body>` close tag in the skeleton
/// 3. Inserting the trailing content just before `</body>`
///
/// If there is no content after `</html>`, or if the structure doesn't match
/// the expected KF8 skeleton pattern, the input is returned unchanged.
fn reassemble_kf8_xhtml<'a>(part: &'a str) -> Cow<'a, str> {
    // Find the closing </html> tag.
    let html_close_pos = match text_utils::find_ci(part.as_bytes(), b"</html") {
        Some(pos) => pos,
        None => return Cow::Borrowed(part),
    };

    // Find the end of the </html...> tag.
    let html_tag_end = match part[html_close_pos..].find('>') {
        Some(offset) => html_close_pos + offset + 1,
        None => return Cow::Borrowed(part),
    };

    // Extract trailing content after </html>.
    let trailing = part[html_tag_end..].trim();
    if trailing.is_empty() {
        // No misplaced content; the document is already well-formed.
        return Cow::Borrowed(part);
    }

    // We have content after </html> that needs to be moved inside <body>.
    let skeleton = &part[..html_tag_end];

    // Find the </body> close tag in the skeleton.
    let body_close_pos = match text_utils::find_ci(&part.as_bytes()[..html_tag_end], b"</body") {
        Some(pos) => pos,
        None => {
            // No </body> tag: fall back to inserting before </html>.
            let mut result = String::with_capacity(part.len());
            result.push_str(&part[..html_close_pos]);
            result.push('\n');
            result.push_str(trailing);
            result.push('\n');
            result.push_str(&part[html_close_pos..html_tag_end]);
            return Cow::Owned(result);
        },
    };

    // Build the fixed XHTML: skeleton up to </body>, then trailing content,
    // then </body></html>.
    let mut result = String::with_capacity(part.len());
    result.push_str(&skeleton[..body_close_pos]);
    result.push('\n');
    result.push_str(trailing);
    result.push('\n');
    result.push_str(&skeleton[body_close_pos..]);
    Cow::Owned(result)
}

/// Splits HTML content at XHTML document boundaries (`<?xml` declarations).
///
/// Returns a vector of string slices, each being a complete XHTML document.
/// If the content does not contain multiple `<?xml` declarations, falls back
/// to looking for multiple `<html` tags as boundaries.
fn split_on_xhtml_boundaries<'a>(html: &'a str) -> Vec<&'a str> {
    let xml_positions = find_all_tags(html.as_bytes(), b"<?xml");
    if xml_positions.len() > 1 {
        return split_at_positions(html, &xml_positions);
    }

    let html_positions = find_all_tags(html.as_bytes(), b"<html");
    if html_positions.len() > 1 {
        return split_at_positions(html, &html_positions);
    }

    vec![html]
}

/// Splits a string at the given byte positions. Each position becomes the start
/// of a new part. Content before the first position (if any) is included as the
/// first part only if non-empty.
fn split_at_positions<'a>(html: &'a str, positions: &[usize]) -> Vec<&'a str> {
    if positions.is_empty() {
        return vec![html];
    }
    let mut parts = Vec::with_capacity(positions.len() + 1);

    // If there's content before the first boundary, include it.
    if positions[0] > 0 {
        let prefix = html[..positions[0]].trim();
        if !prefix.is_empty() {
            parts.push(&html[..positions[0]]);
        }
    }

    for i in 0..positions.len() {
        let start = positions[i];
        let end = if i + 1 < positions.len() {
            positions[i + 1]
        } else {
            html.len()
        };
        if start < end {
            parts.push(&html[start..end]);
        }
    }

    if parts.is_empty() {
        parts.push(html);
    }

    parts
}

/// Splits MOBI HTML content into chapters.
///
/// MOBI files use `<mbp:pagebreak />` or `<a filepos=...>` for chapter breaks.
/// This is a simplified splitter that looks for common patterns.
fn split_mobi_content<'a>(html: &'a str) -> Vec<SimpleChapter<'a>> {
    let mut chapters = Vec::new();

    let parts: Vec<&str> = split_on_pagebreaks(html);

    if parts.len() <= 1 {
        chapters.push(SimpleChapter {
            title: None,
            content: Cow::Borrowed(html),
        });
        return chapters;
    }

    let mut untitled_counter = 0usize;
    for part in parts.iter() {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }

        let title = extract_first_heading(trimmed)
            .or_else(|| extract_fallback_title(trimmed))
            .map(|t| sanitize_toc_label(&t).into_owned())
            .or_else(|| {
                untitled_counter += 1;
                Some(format!("Untitled Section {}", untitled_counter))
            });

        chapters.push(SimpleChapter {
            title,
            content: Cow::Borrowed(trimmed),
        });
    }

    if chapters.is_empty() {
        chapters.push(SimpleChapter {
            title: None,
            content: Cow::Borrowed(html),
        });
    }

    chapters
}

/// A simple chapter extracted from MOBI content.
struct SimpleChapter<'a> {
    title: Option<String>,
    content: Cow<'a, str>,
}

/// Splits HTML on `<mbp:pagebreak` tags.
fn split_on_pagebreaks<'a>(html: &'a str) -> Vec<&'a str> {
    let needle = b"<mbp:pagebreak";
    let positions = find_all_tags(html.as_bytes(), needle);

    if positions.is_empty() {
        return vec![html];
    }

    let mut parts = Vec::with_capacity(positions.len() + 1);
    let mut last = 0;

    for idx in positions {
        if idx > last {
            parts.push(&html[last..idx]);
        }
        // Find the end of this tag in the original.
        if let Some(end) = html[idx..].find('>') {
            last = idx + end + 1;
        } else {
            last = idx + needle.len();
        }
    }

    if last < html.len() {
        parts.push(&html[last..]);
    }

    if parts.is_empty() {
        parts.push(html);
    }

    parts
}

/// Extracts the text content of the first `<h1>`, `<h2>`, or `<h3>` tag.
///
/// Uses a single-pass scan: finds `<` via memchr, then checks the next bytes
/// for `h1`/`h2`/`h3` (case-insensitive). This replaces the previous approach
/// of 3 separate case-insensitive full-text searches.
fn extract_first_heading(html: &str) -> Option<String> {
    let bytes = html.as_bytes();
    let mut pos = 0;
    while let Some(lt_offset) = memchr::memchr(b'<', &bytes[pos..]) {
        let abs = pos + lt_offset;
        // Need at least 3 more bytes after '<': e.g. "h1>" or "h1 "
        if abs + 3 < bytes.len() {
            let next = bytes[abs + 1] | 0x20; // ASCII lowercase
            let digit = bytes[abs + 2];
            if next == b'h' && (digit == b'1' || digit == b'2' || digit == b'3') {
                // Verify it's actually a tag (next char must be '>', ' ', or another
                // attribute-starting character, not e.g. "html" or "href").
                let after_digit = bytes[abs + 3];
                if after_digit == b'>'
                    || after_digit == b' '
                    || after_digit == b'\t'
                    || after_digit == b'\n'
                    || after_digit == b'\r'
                    || after_digit == b'/'
                {
                    // Found a heading open tag. Find its '>' to get content start.
                    let tag_end_rel = bytes[abs..].iter().position(|&b| b == b'>')?;
                    let content_start = abs + tag_end_rel + 1;
                    // Build the close tag pattern: </hN (lowercase)
                    let close_pattern = [b'<', b'/', b'h', digit];
                    let content_end = text_utils::find_ci(&bytes[content_start..], &close_pattern)
                        .map(|p| content_start + p)?;
                    let heading_html = &html[content_start..content_end];
                    let text = strip_tags(heading_html).trim().to_string();
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
        }
        pos = abs + 1;
    }
    None
}

/// Extracts a meaningful title from XHTML content when no heading is found.
///
/// The extraction strategy (in priority order):
/// 1. The `<title>` tag content (if present and not a generic site title)
/// 2. The `alt` attribute of the first `<img>` in the body (if non-empty)
/// 3. The first few words of visible body text
///
/// Returns `None` if no meaningful text can be extracted.
fn extract_fallback_title(html: &str) -> Option<String> {
    // Strategy 1: Try the <title> tag.
    if let Some(title) = extract_title_tag(html) {
        let cleaned = sanitize_toc_label(&title);
        // Skip generic/site-level titles.
        if !cleaned.is_empty()
            && !cleaned.eq_ignore_ascii_case("unknown")
            && !cleaned.contains("Project Gutenberg")
            && !cleaned.contains('|')
        {
            return Some(cleaned.into_owned());
        }
    }

    // Strategy 2: Try alt text from the first <img> in the body.
    if let Some(alt) = extract_first_img_alt(html) {
        let cleaned = sanitize_toc_label(&alt);
        if !cleaned.is_empty() {
            return Some(truncate_title(&cleaned, 80));
        }
    }

    // Strategy 3: Extract first significant text from body content.
    if let Some(snippet) = extract_body_text_snippet(html) {
        let cleaned = sanitize_toc_label(&snippet);
        if !cleaned.is_empty() {
            return Some(truncate_title(&cleaned, 80));
        }
    }

    None
}

/// Extracts the content of the `<title>` tag from an HTML document.
fn extract_title_tag(html: &str) -> Option<String> {
    let start = text_utils::find_ci(html.as_bytes(), b"<title")?;
    let content_start = html[start..].find('>')? + start + 1;
    let content_end =
        text_utils::find_ci(&html.as_bytes()[content_start..], b"</title")? + content_start;
    let raw = &html[content_start..content_end];
    let text = strip_tags(raw).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

/// Extracts the `alt` attribute from the first `<img>` tag in the body.
fn extract_first_img_alt(html: &str) -> Option<String> {
    let bytes = html.as_bytes();

    // Only look inside <body>.
    let body_start = text_utils::find_ci(bytes, b"<body")?;
    let body_html = &html[body_start..];

    let img_pos = text_utils::find_ci(body_html.as_bytes(), b"<img ")?;
    let img_end = body_html[img_pos..].find('>')?;
    let img_tag = &bytes[body_start + img_pos..body_start + img_pos + img_end + 1];

    extract_attr_value(img_tag, b"alt")
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
}

/// Extracts the first few words of visible text from the body content.
fn extract_body_text_snippet(html: &str) -> Option<String> {
    // Find <body>.
    let body_start = text_utils::find_ci(html.as_bytes(), b"<body")?;
    let body_tag_end = html[body_start..].find('>')? + body_start + 1;

    let body_close = text_utils::find_ci(&html.as_bytes()[body_tag_end..], b"</body")
        .map(|pos| body_tag_end + pos)
        .unwrap_or(html.len());

    let body_html = &html[body_tag_end..body_close];
    let text = strip_tags(body_html);
    let text = sanitize_toc_label(&text);

    if text.is_empty() {
        return None;
    }

    Some(truncate_title(&text, 60))
}

/// Truncates a title to a maximum number of characters, breaking at a word boundary.
/// Appends "..." if truncated.
fn truncate_title(s: &str, max_chars: usize) -> String {
    // Find the byte offset at the max_chars-th character boundary.
    // Returns None when the string has <= max_chars characters.
    let byte_limit = match s.char_indices().nth(max_chars).map(|(i, _)| i) {
        Some(b) => b,
        None => return s.to_string(),
    };
    let truncated = &s[..byte_limit];
    // Break at the last space if it's not too early in the string.
    if let Some(last_space) = truncated.rfind(' ')
        && last_space > byte_limit / 3
    {
        return format!("{}...", &s[..last_space]);
    }
    format!("{}...", truncated)
}

/// Normalizes a TOC label to a clean single-line string.
///
/// - Replaces all newline characters (`\n`, `\r\n`, `\r`) with a single space
/// - Collapses multiple consecutive spaces to one
/// - Trims leading/trailing whitespace
fn sanitize_toc_label(label: &str) -> Cow<'_, str> {
    // Fast path: check whether any sanitization is actually needed.
    // A label is clean when it has no leading/trailing whitespace,
    // no newlines/tabs/carriage-returns, and no consecutive spaces.
    let needs_sanitize = {
        let bytes = label.as_bytes();
        if bytes.is_empty() {
            false
        } else {
            let first = bytes[0];
            let last = bytes[bytes.len() - 1];
            first == b' '
                || first == b'\t'
                || first == b'\n'
                || first == b'\r'
                || last == b' '
                || last == b'\t'
                || last == b'\n'
                || last == b'\r'
                || bytes.windows(2).any(|w| {
                    let a = w[0];
                    let b = w[1];
                    a == b'\n'
                        || a == b'\r'
                        || a == b'\t'
                        || (a == b' ' && (b == b' ' || b == b'\n' || b == b'\r' || b == b'\t'))
                })
        }
    };

    if !needs_sanitize {
        return Cow::Borrowed(label);
    }

    let mut result = String::with_capacity(label.len());
    let mut last_was_space = true; // start true to trim leading whitespace

    for ch in label.chars() {
        if ch == '\n' || ch == '\r' || ch == '\t' || ch == ' ' {
            if !last_was_space {
                result.push(' ');
                last_was_space = true;
            }
        } else {
            result.push(ch);
            last_was_space = false;
        }
    }

    // Trim trailing space.
    if result.ends_with(' ') {
        result.pop();
    }

    Cow::Owned(result)
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
            "test",
            b"BOOK",
            b"MOBI",
            1,
            &[88], // offset after header
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

        let pdb_data =
            crate::formats::common::palm_db::build_pdb_header("test", b"BOOK", b"MOBI", 1, &[88]);
        let mut full_data = pdb_data;
        full_data.extend_from_slice(&data);

        let pdb = PdbFile::parse(full_data).unwrap();
        assert!(parse_fdst(&pdb, 0).is_none());
    }

    // --- kindle: reference resolution tests ---

    #[test]
    fn resolve_kindle_embed_basic() {
        let image_paths = vec!["images/0.jpg".to_string(), "images/1.png".to_string()];
        let flow_paths: Vec<Option<String>> = vec![];

        // kindle:embed:0001 is 1-based, so index 1 maps to image_paths[0]
        let html = r#"<img src="kindle:embed:0001?mime=image/jpeg">"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths, 0);
        assert_eq!(result, r#"<img src="images/0.jpg">"#);
    }

    #[test]
    fn resolve_kindle_embed_second_image() {
        let image_paths = vec!["images/0.jpg".to_string(), "images/1.png".to_string()];
        let flow_paths: Vec<Option<String>> = vec![];

        // kindle:embed:0002 (1-based) maps to image_paths[1]
        let html = r#"<img src="kindle:embed:0002?mime=image/png">"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths, 0);
        assert_eq!(result, r#"<img src="images/1.png">"#);
    }

    #[test]
    fn resolve_kindle_embed_no_query() {
        let image_paths = vec!["images/0.jpg".to_string()];
        let flow_paths: Vec<Option<String>> = vec![];

        // kindle:embed:0001 (1-based) → image_paths[0]
        let html = r#"<img src="kindle:embed:0001">"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths, 0);
        assert_eq!(result, r#"<img src="images/0.jpg">"#);
    }

    #[test]
    fn resolve_kindle_embed_zero_left_as_is() {
        // kindle:embed:0000 decodes to 0, which is invalid for 1-based indexing
        let image_paths = vec!["images/0.jpg".to_string()];
        let flow_paths: Vec<Option<String>> = vec![];

        let html = r#"<img src="kindle:embed:0000?mime=image/jpeg">"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths, 0);
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
        let result = resolve_kindle_references(html, &image_paths, &flow_paths, 0);
        assert_eq!(result, r#"<link href="flows/flow_1.css">"#);
    }

    #[test]
    fn resolve_kindle_flow_base32() {
        // Regression: kindle:flow uses base-32 encoding (0-9, A-V), not decimal.
        // 000A in base-32 = 10 in decimal.
        let image_paths: Vec<String> = vec![];
        let mut flow_paths: Vec<Option<String>> = vec![None]; // flow 0 = main
        for i in 1..=10 {
            flow_paths.push(Some(format!("flows/flow_{}.css", i)));
        }

        let html = r#"<link href="kindle:flow:000A?mime=text/css">"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths, 0);
        assert_eq!(result, r#"<link href="flows/flow_10.css">"#);
    }

    #[test]
    fn resolve_kindle_mixed_references() {
        let image_paths = vec!["images/0.jpg".to_string(), "images/1.png".to_string()];
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
        let result = resolve_kindle_references(html, &image_paths, &flow_paths, 0);
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
        let result = resolve_kindle_references(html, &image_paths, &flow_paths, 0);
        // Should be left unchanged since index is out of range
        assert_eq!(result, html);
    }

    #[test]
    fn resolve_kindle_no_references() {
        let image_paths: Vec<String> = vec![];
        let flow_paths: Vec<Option<String>> = vec![];

        let html = "<p>No kindle references here</p>";
        let result = resolve_kindle_references(html, &image_paths, &flow_paths, 0);
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

    // --- KF8 XHTML splitting tests ---

    #[test]
    fn split_kf8_content_multiple_xml_docs() {
        let html = concat!(
            "<?xml version=\"1.0\"?><html><body><p>Chapter 1</p></body></html>",
            "<?xml version=\"1.0\"?><html><body><p>Chapter 2</p></body></html>",
            "<?xml version=\"1.0\"?><html><body><p>Chapter 3</p></body></html>",
        );
        let chapters = split_kf8_content(html);
        assert_eq!(chapters.len(), 3, "Should split into 3 chapters");
        assert!(chapters[0].content.contains("Chapter 1"));
        assert!(chapters[1].content.contains("Chapter 2"));
        assert!(chapters[2].content.contains("Chapter 3"));
    }

    #[test]
    fn split_kf8_content_each_part_is_complete() {
        let doc1 = "<?xml version=\"1.0\"?><html><body><p>Part 1</p></body></html>";
        let doc2 = "<?xml version=\"1.0\"?><html><body><p>Part 2</p></body></html>";
        let html = format!("{}{}", doc1, doc2);
        let chapters = split_kf8_content(&html);
        assert_eq!(chapters.len(), 2);
        // Each part should start with <?xml
        assert!(chapters[0].content.starts_with("<?xml"));
        assert!(chapters[1].content.starts_with("<?xml"));
        // Each part should have exactly one <html> root
        assert_eq!(chapters[0].content.matches("<html>").count(), 1);
        assert_eq!(chapters[1].content.matches("<html>").count(), 1);
    }

    #[test]
    fn split_kf8_content_single_doc_falls_back() {
        let html = "<?xml version=\"1.0\"?><html><body><p>Only one doc</p></body></html>";
        let chapters = split_kf8_content(html);
        // Single document with no pagebreaks should produce 1 chapter.
        assert_eq!(chapters.len(), 1);
        assert!(chapters[0].content.contains("Only one doc"));
    }

    #[test]
    fn split_kf8_content_no_xml_decl_multiple_html() {
        // Some KF8 may have multiple <html> without <?xml declarations.
        let html = "<html><body><p>Part 1</p></body></html><html><body><p>Part 2</p></body></html>";
        let chapters = split_kf8_content(html);
        assert_eq!(chapters.len(), 2, "Should split on <html> boundaries");
        assert!(chapters[0].content.contains("Part 1"));
        assert!(chapters[1].content.contains("Part 2"));
    }

    #[test]
    fn split_kf8_content_with_headings() {
        let html = concat!(
            "<?xml version=\"1.0\"?><html><body><h1>Introduction</h1><p>Intro text</p></body></html>",
            "<?xml version=\"1.0\"?><html><body><h2>Chapter One</h2><p>Content</p></body></html>",
        );
        let chapters = split_kf8_content(html);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].title.as_deref(), Some("Introduction"));
        assert_eq!(chapters[1].title.as_deref(), Some("Chapter One"));
    }

    #[test]
    fn split_on_xhtml_boundaries_preserves_content() {
        let doc1 = "<?xml version=\"1.0\"?>\n<html>\n<body>\n<p>First</p>\n</body>\n</html>\n";
        let doc2 = "<?xml version=\"1.0\"?>\n<html>\n<body>\n<p>Second</p>\n</body>\n</html>\n";
        let combined = format!("{}{}", doc1, doc2);
        let parts = split_on_xhtml_boundaries(&combined);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], doc1);
        assert_eq!(parts[1], doc2);
    }

    // --- kindle:pos:fid resolution tests ---

    #[test]
    fn resolve_kindle_pos_fid_basic() {
        let image_paths: Vec<String> = vec![];
        let flow_paths: Vec<Option<String>> = vec![];

        // kindle:pos:fid:0000:off:0000000000 -> chapter 0
        let html = r#"<a href="kindle:pos:fid:0000:off:0000000000">Link</a>"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths, 10);
        assert_eq!(result, r#"<a href="mobi_ch_0.xhtml">Link</a>"#);
    }

    #[test]
    fn resolve_kindle_pos_fid_chapter_5() {
        let image_paths: Vec<String> = vec![];
        let flow_paths: Vec<Option<String>> = vec![];

        // kindle:pos:fid:0005:off:0000000000 -> fid=5, chapter 5
        let html = r#"<a href="kindle:pos:fid:0005:off:0000000000">Link</a>"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths, 10);
        assert_eq!(result, r#"<a href="mobi_ch_5.xhtml">Link</a>"#);
    }

    #[test]
    fn resolve_kindle_pos_fid_base32() {
        let image_paths: Vec<String> = vec![];
        let flow_paths: Vec<Option<String>> = vec![];

        // kindle:pos:fid:000A:off:0000000100 -> fid=10 (A in base32)
        let html = r#"<a href="kindle:pos:fid:000A:off:0000000100">Link</a>"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths, 20);
        assert_eq!(result, r#"<a href="mobi_ch_10.xhtml">Link</a>"#);
    }

    #[test]
    fn resolve_kindle_pos_fid_out_of_range_left_unchanged() {
        let image_paths: Vec<String> = vec![];
        let flow_paths: Vec<Option<String>> = vec![];

        // fid=99 but only 10 chapters: left unchanged rather than misdirecting.
        let html = r#"<a href="kindle:pos:fid:0033:off:0000000000">Link</a>"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths, 10);
        assert_eq!(result, html);
    }

    #[test]
    fn resolve_kindle_pos_fid_zero_chapters() {
        let image_paths: Vec<String> = vec![];
        let flow_paths: Vec<Option<String>> = vec![];

        // With 0 chapters, pos:fid cannot be resolved; left as-is.
        let html = r#"<a href="kindle:pos:fid:0000:off:0000000000">Link</a>"#;
        let result = resolve_kindle_references(html, &image_paths, &flow_paths, 0);
        assert_eq!(result, html);
    }

    #[test]
    fn resolve_kindle_pos_fid_mixed_with_embed() {
        let image_paths = vec!["images/0.jpg".to_string()];
        let flow_paths: Vec<Option<String>> = vec![];

        let html = concat!(
            r#"<a href="kindle:pos:fid:0002:off:0000000000">Chapter link</a>"#,
            r#"<img src="kindle:embed:0001?mime=image/jpeg"/>"#,
        );
        let result = resolve_kindle_references(html, &image_paths, &flow_paths, 5);
        assert_eq!(
            result,
            concat!(
                r#"<a href="mobi_ch_2.xhtml">Chapter link</a>"#,
                r#"<img src="images/0.jpg"/>"#,
            )
        );
    }

    #[test]
    fn try_resolve_pos_fid_parsing() {
        // Verify the parser correctly handles the kindle:pos:fid format
        let after_kindle = "pos:fid:0003:off:0000000042\"rest";
        let result = try_resolve_pos_fid(after_kindle, 10);
        assert!(result.is_some());
        let (replacement, consumed) = result.unwrap();
        assert_eq!(replacement, "mobi_ch_3.xhtml");
        // consumed = "pos:fid:" (8) + "0003" (4) + ":off:" (5) + "0000000042" (10) = 27
        assert_eq!(consumed, 27);
    }

    #[test]
    fn find_all_case_insensitive_basic() {
        let haystack = b"<?xml one><?xml two><?XML three>";
        let positions = text_utils::find_all_case_insensitive(haystack, b"<?xml");
        assert_eq!(positions.len(), 3);
        assert_eq!(positions[0], 0);
        assert_eq!(positions[1], 10);
        assert_eq!(positions[2], 20);
    }

    // --- KF8 XHTML body reassembly tests ---

    #[test]
    fn reassemble_kf8_xhtml_moves_content_into_body() {
        // Simulates the KF8 skeleton/fragment pattern: empty body followed by
        // content after </html>.
        let input = concat!(
            "<?xml version=\"1.0\"?>",
            "<html><head><title>T</title></head>",
            "<body>\n</body></html>",
            "<h1>Chapter</h1><p>Content here</p>",
        );
        let result = reassemble_kf8_xhtml(input);

        // Content must be inside <body>.
        assert!(
            result.contains("<body>\n\n<h1>Chapter</h1><p>Content here</p>\n</body>"),
            "Content should be inside <body>. Got:\n{}",
            result
        );
        // Nothing after </html>.
        let html_close = result.find("</html>").unwrap();
        let after = result[html_close + 7..].trim();
        assert!(
            after.is_empty(),
            "Nothing should appear after </html>. Got: {}",
            after
        );
    }

    #[test]
    fn reassemble_kf8_xhtml_preserves_wellformed_doc() {
        // A well-formed document (content already in body) should be unchanged.
        let input = "<?xml version=\"1.0\"?><html><body><p>Content</p></body></html>";
        let result = reassemble_kf8_xhtml(input);
        assert_eq!(result, input, "Well-formed document should not be modified");
    }

    #[test]
    fn reassemble_kf8_xhtml_no_html_close() {
        // No </html> tag at all -- return as-is.
        let input = "<body><p>Content</p></body>";
        let result = reassemble_kf8_xhtml(input);
        assert_eq!(result, input);
    }

    #[test]
    fn reassemble_kf8_xhtml_with_body_attributes() {
        // Body tag with class and aid attributes (common in KF8).
        let input = concat!(
            "<?xml version=\"1.0\"?>",
            "<html xmlns=\"http://www.w3.org/1999/xhtml\">",
            "<head><title>T</title></head>",
            "<body class=\"myclass\" aid=\"ABC1\">\n</body></html>",
            "<h2>Title</h2><p>Paragraph</p>",
        );
        let result = reassemble_kf8_xhtml(input);

        assert!(
            result.contains("<h2>Title</h2>"),
            "Result should contain heading"
        );
        assert!(
            result.contains("<p>Paragraph</p>"),
            "Result should contain paragraph"
        );

        // Verify well-formedness: body content is before </body>.
        let body_close = result.find("</body>").unwrap();
        let html_close = result.find("</html>").unwrap();
        assert!(
            body_close < html_close,
            "</body> should come before </html>"
        );
        assert!(
            result.find("<h2>Title</h2>").unwrap() < body_close,
            "Content should be before </body>"
        );
    }

    #[test]
    fn split_kf8_content_fixes_body_placement() {
        // Simulate concatenated KF8 documents with empty bodies and trailing content.
        let html = concat!(
            "<?xml version=\"1.0\"?><html><body>\n</body></html>",
            "<h1>Chapter 1</h1><p>Text 1</p>",
            "<?xml version=\"1.0\"?><html><body>\n</body></html>",
            "<h2>Chapter 2</h2><p>Text 2</p>",
        );
        let chapters = split_kf8_content(html);
        assert_eq!(chapters.len(), 2, "Should split into 2 chapters");

        // Each chapter's content should be well-formed with content inside <body>.
        for (i, ch) in chapters.iter().enumerate() {
            let html_close = ch.content.find("</html>");
            assert!(html_close.is_some(), "Chapter {} should have </html>", i);
            let after = ch.content[html_close.unwrap() + 7..].trim();
            assert!(
                after.is_empty(),
                "Chapter {} should have no content after </html>. Got: {}",
                i,
                after
            );

            let body_start = ch.content.find("<body").expect("Should have <body>");
            let body_close = ch.content.find("</body>").expect("Should have </body>");
            let body_tag_end = ch.content[body_start..].find('>').unwrap() + body_start + 1;
            let body_content = &ch.content[body_tag_end..body_close];
            assert!(
                !body_content.trim().is_empty(),
                "Chapter {} should have content inside <body>",
                i
            );
        }

        assert!(chapters[0].content.contains("Chapter 1"));
        assert!(chapters[1].content.contains("Chapter 2"));
    }

    // --- sanitize_toc_label tests ---

    #[test]
    fn sanitize_toc_label_collapses_newlines() {
        let input = "I hope Mr. Bingley will like it.\n\nCHAPTER II.";
        let result = sanitize_toc_label(input);
        assert_eq!(result, "I hope Mr. Bingley will like it. CHAPTER II.");
        assert!(!result.contains('\n'));
    }

    #[test]
    fn sanitize_toc_label_collapses_crlf() {
        let input = "Line one.\r\nLine two.\r\nLine three.";
        let result = sanitize_toc_label(input);
        assert_eq!(result, "Line one. Line two. Line three.");
    }

    #[test]
    fn sanitize_toc_label_trims_whitespace() {
        let input = "  Hello World  ";
        assert_eq!(sanitize_toc_label(input), "Hello World");
    }

    #[test]
    fn sanitize_toc_label_collapses_spaces() {
        let input = "PRIDE.   and   PREJUDICE";
        assert_eq!(sanitize_toc_label(input), "PRIDE. and PREJUDICE");
    }

    #[test]
    fn sanitize_toc_label_empty_input() {
        assert_eq!(sanitize_toc_label(""), "");
        assert_eq!(sanitize_toc_label("  \n  "), "");
    }

    // --- extract_fallback_title tests ---

    #[test]
    fn extract_fallback_title_from_title_tag() {
        let html = r#"<html><head><title>"Cover"</title></head><body></body></html>"#;
        let result = extract_fallback_title(html);
        assert_eq!(result, Some("\"Cover\"".to_string()));
    }

    #[test]
    fn extract_fallback_title_from_img_alt() {
        let html = r#"<html><head><title>Pride and prejudice | Project Gutenberg</title></head>
        <body><img alt="Dedication page" src="img.jpg"/></body></html>"#;
        let result = extract_fallback_title(html);
        assert_eq!(result, Some("Dedication page".to_string()));
    }

    #[test]
    fn extract_fallback_title_from_body_text() {
        let html = r#"<html><head><title>Pride and prejudice | Project Gutenberg</title></head>
        <body><p>Some interesting text content here.</p></body></html>"#;
        let result = extract_fallback_title(html);
        assert!(result.is_some());
        assert!(result.unwrap().starts_with("Some interesting text"));
    }

    #[test]
    fn extract_fallback_title_skips_generic_titles() {
        // Title with "Project Gutenberg" should be skipped.
        let html = r#"<html><head><title>Book | Project Gutenberg</title></head>
        <body><img alt="" src="x.jpg"/><p>First line.</p></body></html>"#;
        let result = extract_fallback_title(html);
        // Should NOT return the generic title, should fall through to body text.
        assert!(result.is_some());
        assert!(result.unwrap().starts_with("First line"));
    }

    // --- truncate_title tests ---

    #[test]
    fn truncate_title_short_string() {
        assert_eq!(truncate_title("Hello", 80), "Hello");
    }

    #[test]
    fn truncate_title_long_string() {
        let long =
            "This is a very long title that exceeds the maximum character limit for truncation";
        let result = truncate_title(long, 40);
        assert!(result.len() <= 43); // 40 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_title_cjk_does_not_panic() {
        // 60 chars limit on a string of 3-byte CJK characters must not split mid-char.
        let cjk = "目录 科普袖珍馆：昆虫记 科普袖珍馆：爱因斯坦自述 科普袖珍馆：趣味物理学 科普袖珍馆：生命是什么 科普袖珍馆：达尔文笔记 科普袖珍馆：十万个为什么";
        let result = truncate_title(cjk, 60);
        assert!(result.ends_with("..."));
        assert!(result.chars().count() <= 63); // 60 + "..."
    }

    #[test]
    fn truncate_title_empty_string() {
        assert_eq!(truncate_title("", 60), "");
    }

    #[test]
    fn truncate_title_exactly_at_limit() {
        // Exactly 60 CJK chars — should NOT be truncated.
        let s: String = "字".repeat(60);
        assert_eq!(truncate_title(&s, 60), s);
    }

    #[test]
    fn truncate_title_one_over_limit() {
        // 61 CJK chars — truncated to 60 + "...".
        let s: String = "字".repeat(61);
        let result = truncate_title(&s, 60);
        assert!(result.ends_with("..."));
        // No spaces, so it truncates at char boundary directly.
        assert_eq!(result.chars().count(), 63); // 60 + "..."
    }

    #[test]
    fn truncate_title_emoji_4byte() {
        // 4-byte emoji characters should not cause panics.
        let s = "🎉🎊🎈🎁🎂🎃🎄🎅🎆🎇🧨✨🎋🎍🎎🎏🎐🎑🎒🎓🎖🎗🎘🎙🎚🎛🎞🎟🎠🎡🎢🎣🎤🎥🎦🎧🎨🎩🎪🎫🎬🎭🎮🎯🎰🎱🎲🎳🎴🎵🎶🎷🎸🎹🎺🎻🎼🎽🎾🎿🏀";
        let result = truncate_title(s, 60);
        assert!(result.ends_with("..."));
        assert!(result.chars().count() <= 63);
    }

    #[test]
    fn truncate_title_mixed_ascii_cjk() {
        let s = "Hello 你好世界 World 再见朋友 Goodbye 晚安大家";
        let result = truncate_title(s, 10);
        assert!(result.ends_with("..."));
        // Should break at a word boundary (space).
        assert!(result.chars().count() <= 13);
    }

    #[test]
    fn truncate_title_pure_cjk_no_spaces() {
        // No word boundary to break at.
        let s: String = "测试中文标题无空格字符串".to_string();
        let result = truncate_title(&s, 5);
        assert_eq!(result, "测试中文标...");
    }

    #[test]
    fn truncate_title_max_chars_zero() {
        let result = truncate_title("Hello", 0);
        assert_eq!(result, "...");
    }

    #[test]
    fn truncate_title_max_chars_one() {
        let result = truncate_title("Hello World", 1);
        assert_eq!(result, "H...");
    }

    // --- split_kf8_content no generic "Part N" labels ---

    #[test]
    fn split_kf8_no_heading_gets_fallback_not_part_n() {
        let html = concat!(
            "<?xml version=\"1.0\"?><html><head><title>\"Cover\"</title></head><body><img alt=\"Cover\" src=\"cover.jpg\"/></body></html>",
            "<?xml version=\"1.0\"?><html><body><h1>Chapter One</h1><p>Content</p></body></html>",
        );
        let chapters = split_kf8_content(html);
        assert_eq!(chapters.len(), 2);
        // First chapter has no heading, should NOT be "Part 1"
        let title0 = chapters[0].title.as_deref().unwrap();
        assert!(
            !title0.starts_with("Part "),
            "Title should not be generic 'Part N', got: {}",
            title0
        );
        // Second chapter has a heading
        assert_eq!(chapters[1].title.as_deref(), Some("Chapter One"));
    }
}
