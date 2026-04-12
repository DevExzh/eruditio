//! CHM (Compiled HTML Help) format reader.
//!
//! CHM files use the ITSF container: an ITSF header → ITSP directory → PMGL
//! listing blocks with encint-encoded entries. Content lives in section 0
//! (uncompressed) or section 1 (LZX-compressed via MSCompressed transform).

use ahash::AHashMap as HashMap;
use std::io::Read;

use crate::domain::{Book, Chapter, FormatReader};
use crate::error::{EruditioError, Result};
use crate::formats::common::MAX_INPUT_SIZE;
use crate::formats::common::itss::{self, DirectoryEntry};
use crate::formats::common::text_utils;

/// Maximum number of directory entries to prevent DoS from crafted files.
const MAX_ENTRIES: usize = 1_000_000;

// Well-known internal paths
const RESET_TABLE_PATH: &str = "::DataSpace/Storage/MSCompressed/Transform/\
    {7FC28940-9D31-11D0-9B27-00A0C91E9C7C}/InstanceData/ResetTable";
const CONTROL_DATA_PATH: &str = "::DataSpace/Storage/MSCompressed/ControlData";
const CONTENT_PATH: &str = "::DataSpace/Storage/MSCompressed/Content";

// /#SYSTEM entry codes
const SYSTEM_CONTENTS_FILE: u16 = 0;
const SYSTEM_DEFAULT_TOPIC: u16 = 2;
const SYSTEM_TITLE: u16 = 3;

// ---------------------------------------------------------------------------
// CHM Container
// ---------------------------------------------------------------------------

/// Parsed CHM container providing random access to files.
struct ChmContainer {
    data: Vec<u8>,
    entries: HashMap<String, DirectoryEntry>,
    data_offset: u64,
    /// Lazily decompressed MSCompressed section.
    decompressed: Option<Vec<u8>>,
}

impl ChmContainer {
    /// Parse a CHM file from raw bytes.
    fn parse(data: Vec<u8>) -> Result<Self> {
        if data.len() < 0x60 {
            return Err(EruditioError::Format("CHM file too short".into()));
        }

        // --- ITSF header ---
        if &data[0..4] != b"ITSF" {
            return Err(EruditioError::Format(
                "Not a valid CHM file (missing ITSF)".into(),
            ));
        }
        let version = itss::i32_le(&data[4..]);
        if version != 2 && version != 3 {
            return Err(EruditioError::Format(format!(
                "Unsupported ITSF version: {version}"
            )));
        }
        let header_len = usize::try_from(itss::i32_le(&data[8..]))
            .map_err(|_| EruditioError::Format("CHM: negative header length".into()))?;
        let dir_offset = usize::try_from(itss::u64_le(&data[0x48..]))
            .map_err(|_| EruditioError::Format("CHM: directory offset too large".into()))?;
        let dir_len = usize::try_from(itss::u64_le(&data[0x50..]))
            .map_err(|_| EruditioError::Format("CHM: directory length too large".into()))?;
        let data_offset = if version == 3 && header_len >= 0x60 {
            itss::u64_le(&data[0x58..])
        } else {
            u64::try_from(dir_offset + dir_len)
                .map_err(|_| EruditioError::Format("CHM: data offset overflow".into()))?
        };

        if dir_offset + 0x54 > data.len() {
            return Err(EruditioError::Format(
                "CHM directory offset out of range".into(),
            ));
        }

        // --- ITSP header (at dir_offset) ---
        let itsp = &data[dir_offset..];
        if &itsp[0..4] != b"ITSP" {
            return Err(EruditioError::Format(
                "Missing ITSP directory header".into(),
            ));
        }
        let itsp_version = itss::i32_le(&itsp[4..]);
        if itsp_version != 1 {
            return Err(EruditioError::Format(format!(
                "Unsupported ITSP version: {itsp_version}"
            )));
        }
        let itsp_header_len = usize::try_from(itss::i32_le(&itsp[8..]))
            .map_err(|_| EruditioError::Format("CHM: negative ITSP header length".into()))?;
        let block_len = itss::u32_le(&itsp[0x10..]) as usize;
        let num_blocks = itss::u32_le(&itsp[0x28..]) as usize;

        // Directory listing blocks start after the ITSP header
        let listing_start = dir_offset + itsp_header_len;
        let listing_end = (dir_offset + dir_len).min(data.len());

        // --- Parse PMGL blocks ---
        let mut entries = HashMap::new();
        for i in 0..num_blocks {
            let block_offset = listing_start + i * block_len;
            if block_offset + block_len > listing_end {
                break;
            }
            let block = &data[block_offset..block_offset + block_len];

            // Only process PMGL (listing) blocks, skip PMGI (index) blocks
            if &block[0..4] != b"PMGL" {
                continue;
            }

            let free_space = itss::u32_le(&block[4..]) as usize;
            // Entry data starts after the 20-byte PMGL header
            let entry_data = &block[20..];
            let entry_data_len = block_len.saturating_sub(20).saturating_sub(free_space);

            if let Ok(block_entries) = itss::parse_listing_entries(entry_data, entry_data_len) {
                entries.extend(block_entries);
                if entries.len() > MAX_ENTRIES {
                    return Err(EruditioError::Format(
                        "CHM: too many directory entries (possible corrupt file)".into(),
                    ));
                }
            }
        }

        Ok(ChmContainer {
            data,
            entries,
            data_offset,
            decompressed: None,
        })
    }

    /// Get a file's raw bytes by internal path.
    fn get_file(&mut self, name: &str) -> Result<Vec<u8>> {
        let entry = self
            .entries
            .get(name)
            .ok_or_else(|| EruditioError::Parse(format!("CHM entry not found: {name}")))?
            .clone();

        if entry.section == 0 {
            // Uncompressed: read directly from data_offset + entry.offset
            let start = self
                .data_offset
                .checked_add(entry.offset)
                .and_then(|v| usize::try_from(v).ok())
                .ok_or_else(|| {
                    EruditioError::Parse(format!("CHM entry '{name}' offset overflow"))
                })?;
            let end = usize::try_from(entry.size)
                .ok()
                .and_then(|sz| start.checked_add(sz))
                .ok_or_else(|| EruditioError::Parse(format!("CHM entry '{name}' size overflow")))?;
            if end > self.data.len() {
                return Err(EruditioError::Parse(format!(
                    "CHM entry '{name}' extends past file end"
                )));
            }
            Ok(self.data[start..end].to_vec())
        } else {
            // Compressed: decompress the MSCompressed section, then slice
            self.ensure_decompressed()?;
            let decompressed = self.decompressed.as_ref().ok_or_else(|| {
                EruditioError::Parse("CHM decompressed data unavailable after decompression".into())
            })?;
            let start = usize::try_from(entry.offset).map_err(|_| {
                EruditioError::Parse(format!("CHM compressed entry '{name}' offset too large"))
            })?;
            let end = usize::try_from(entry.size)
                .ok()
                .and_then(|sz| start.checked_add(sz))
                .ok_or_else(|| {
                    EruditioError::Parse(format!("CHM compressed entry '{name}' size overflow"))
                })?;
            if end > decompressed.len() {
                return Err(EruditioError::Parse(format!(
                    "CHM compressed entry '{name}' extends past decompressed data"
                )));
            }
            Ok(decompressed[start..end].to_vec())
        }
    }

    /// Lazily decompress the MSCompressed section.
    fn ensure_decompressed(&mut self) -> Result<()> {
        if self.decompressed.is_some() {
            return Ok(());
        }

        // Read control data, reset table, and content from section 0
        let control_raw = self.get_section0_file(CONTROL_DATA_PATH)?;
        let reset_raw = self.get_section0_file(RESET_TABLE_PATH)?;
        let content = self.get_section0_file(CONTENT_PATH)?;

        let ctrl = itss::parse_lzxc_control_data_chm(&control_raw)?;
        let reset_table = itss::parse_lzx_reset_table(&reset_raw)?;
        let window_size = itss::window_size_from_bytes(ctrl.window_size)?;

        let decompressed = itss::lzx_decompress_section(&content, window_size, &reset_table)?;
        self.decompressed = Some(decompressed);
        Ok(())
    }

    /// Read a file known to be in section 0 (uncompressed).
    fn get_section0_file(&self, name: &str) -> Result<Vec<u8>> {
        let entry = self
            .entries
            .get(name)
            .ok_or_else(|| EruditioError::Parse(format!("CHM entry not found: {name}")))?;

        let start = self
            .data_offset
            .checked_add(entry.offset)
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| {
                EruditioError::Parse(format!("CHM section 0 entry '{name}' offset overflow"))
            })?;
        let end = usize::try_from(entry.size)
            .ok()
            .and_then(|sz| start.checked_add(sz))
            .ok_or_else(|| {
                EruditioError::Parse(format!("CHM section 0 entry '{name}' size overflow"))
            })?;
        if end > self.data.len() {
            return Err(EruditioError::Parse(format!(
                "CHM section 0 entry '{name}' extends past file end"
            )));
        }
        Ok(self.data[start..end].to_vec())
    }

    /// Iterate over all user content file paths (excluding internal `::` paths).
    fn content_files(&self) -> Vec<&str> {
        self.entries
            .keys()
            .filter(|k| !k.starts_with("::") && !k.starts_with("/#"))
            .map(|s| s.as_str())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// /#SYSTEM metadata parser
// ---------------------------------------------------------------------------

struct ChmSystemInfo {
    title: Option<String>,
    default_topic: Option<String>,
    contents_file: Option<String>,
}

fn parse_system_file(data: &[u8]) -> ChmSystemInfo {
    let mut info = ChmSystemInfo {
        title: None,
        default_topic: None,
        contents_file: None,
    };

    if data.len() < 4 {
        return info;
    }

    // Skip 4-byte version
    let mut pos = 4;
    while pos + 4 <= data.len() {
        let code = itss::u16_le(&data[pos..]);
        let length = itss::u16_le(&data[pos + 2..]) as usize;
        pos += 4;

        if pos + length > data.len() {
            break;
        }
        let value_data = &data[pos..pos + length];
        pos += length;

        // Extract null-terminated string
        let value = extract_cstring(value_data);

        match code {
            SYSTEM_CONTENTS_FILE => info.contents_file = Some(value),
            SYSTEM_DEFAULT_TOPIC => info.default_topic = Some(value),
            SYSTEM_TITLE => info.title = Some(value),
            _ => {},
        }
    }

    info
}

fn extract_cstring(data: &[u8]) -> String {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    crate::formats::common::text_utils::bytes_to_string(&data[..end])
}

// ---------------------------------------------------------------------------
// .hhc (HTML Help TOC) parser
// ---------------------------------------------------------------------------

/// A TOC entry extracted from an .hhc file.
struct HhcEntry {
    name: String,
    local: String,
}

/// Parse an .hhc file to extract TOC entries.
///
/// .hhc files use `<OBJECT>` tags with `<param>` children:
/// ```html
/// <OBJECT type="text/sitemap">
///   <param name="Name" value="Chapter Title">
///   <param name="Local" value="chapter1.htm">
/// </OBJECT>
/// ```
fn parse_hhc(data: &[u8]) -> Vec<HhcEntry> {
    let text = crate::formats::common::text_utils::bytes_to_cow_str(data);
    let lowered = text_utils::ascii_lowercase_copy(text.as_bytes());
    let mut entries = Vec::new();

    // Find each <object type="text/sitemap"> ... </object> block
    let mut search_pos = 0;
    while let Some(obj_offset) = memchr::memmem::find(&lowered[search_pos..], b"<object") {
        let abs_start = search_pos + obj_offset;
        let tag_end = match text[abs_start..].find('>') {
            Some(p) => abs_start + p,
            None => break,
        };
        let tag = &text[abs_start..=tag_end];

        if !tag
            .as_bytes()
            .windows(b"text/sitemap".len())
            .any(|w| w.eq_ignore_ascii_case(b"text/sitemap"))
        {
            search_pos = tag_end + 1;
            continue;
        }

        let obj_end = match memchr::memmem::find(&lowered[tag_end..], b"</object") {
            Some(p) => tag_end + p,
            None => break,
        };

        let block = &text[tag_end + 1..obj_end];
        let block_lowered = &lowered[tag_end + 1..obj_end];

        let mut name = String::new();
        let mut local = String::new();

        // Extract <param> values
        let mut param_pos = 0;
        while let Some(p) = memchr::memmem::find(&block_lowered[param_pos..], b"<param") {
            let param_start = param_pos + p;
            let param_end = match block[param_start..].find('>') {
                Some(pe) => param_start + pe,
                None => break,
            };
            let param_tag = &block[param_start..=param_end];
            let param_lower = param_tag.to_ascii_lowercase();

            if let (Some(n), Some(v)) = (
                extract_attr(&param_lower, param_tag, "name"),
                extract_attr(&param_lower, param_tag, "value"),
            ) {
                match n.to_ascii_lowercase().as_str() {
                    "name" => name = v,
                    "local" => local = v,
                    _ => {},
                }
            }
            param_pos = param_end + 1;
        }

        if !name.is_empty() && !local.is_empty() {
            entries.push(HhcEntry { name, local });
        }

        search_pos = obj_end;
    }

    entries
}

/// Extract an HTML attribute value from a tag string.
/// `tag_lower` is the lowercased version; `tag_orig` preserves original case for values.
fn extract_attr(tag_lower: &str, tag_orig: &str, attr_name: &str) -> Option<String> {
    let pattern = format!("{attr_name}=\"");
    let start = tag_lower.find(&pattern)?;
    let value_start = start + pattern.len();
    let value_end = tag_orig[value_start..].find('"')?;
    Some(tag_orig[value_start..value_start + value_end].to_string())
}

// ---------------------------------------------------------------------------
// Content extraction helpers
// ---------------------------------------------------------------------------

/// Strip `<script>` tags and their content from HTML.
fn strip_scripts(html: &str) -> String {
    let lowered = text_utils::ascii_lowercase_copy(html.as_bytes());
    let mut result = String::with_capacity(html.len());
    let mut pos = 0;

    while let Some(start) = memchr::memmem::find(&lowered[pos..], b"<script") {
        let abs_start = pos + start;
        result.push_str(&html[pos..abs_start]);

        // Find closing </script>
        if let Some(end) = memchr::memmem::find(&lowered[abs_start..], b"</script>") {
            pos = abs_start + end + "</script>".len();
        } else {
            // No closing tag — skip to end
            pos = html.len();
        }
    }
    result.push_str(&html[pos..]);
    result
}

/// Decode bytes to a string, trying UTF-8 first, falling back to cp1252.
fn decode_html(data: &[u8]) -> String {
    match std::str::from_utf8(data) {
        Ok(s) => s.to_string(),
        Err(_) => {
            // Fallback: decode as Windows-1252
            data.iter()
                .map(|&b| {
                    if b < 0x80 {
                        b as char
                    } else {
                        // Windows-1252 to Unicode for 0x80-0xFF range
                        cp1252_to_char(b)
                    }
                })
                .collect()
        },
    }
}

fn cp1252_to_char(b: u8) -> char {
    crate::formats::common::text_utils::cp1252_byte_to_char(b)
}

/// Normalise a CHM path by stripping leading `/` and backslash.
fn normalize_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    p.trim_start_matches('/').to_string()
}

// ---------------------------------------------------------------------------
// ChmReader
// ---------------------------------------------------------------------------

/// CHM (Compiled HTML Help) format reader.
#[derive(Default)]
pub struct ChmReader;

impl ChmReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for ChmReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut buffer = Vec::new();
        (&mut *reader)
            .take(MAX_INPUT_SIZE)
            .read_to_end(&mut buffer)?;

        let mut container = ChmContainer::parse(buffer)?;
        let mut book = Book::new();

        // --- Metadata from /#SYSTEM ---
        let sys_info = if let Ok(sys_data) = container.get_file("/#SYSTEM") {
            parse_system_file(&sys_data)
        } else {
            ChmSystemInfo {
                title: None,
                default_topic: None,
                contents_file: None,
            }
        };

        if let Some(ref title) = sys_info.title
            && !title.is_empty()
        {
            book.metadata.title = Some(title.clone());
        }

        // --- Build chapter list from .hhc or fallback ---
        let hhc_entries = find_and_parse_hhc(&mut container, &sys_info);

        // Collect HTML file paths
        let html_files: Vec<String> = if !hhc_entries.is_empty() {
            hhc_entries.iter().map(|e| e.local.clone()).collect()
        } else {
            // Fallback: enumerate all .html/.htm files
            let mut files: Vec<String> = container
                .content_files()
                .into_iter()
                .filter(|f| {
                    let lower = f.to_ascii_lowercase();
                    lower.ends_with(".html") || lower.ends_with(".htm")
                })
                .map(String::from)
                .collect();
            files.sort();
            files
        };

        // --- Extract chapters ---
        for (index, path) in html_files.iter().enumerate() {
            let norm = normalize_path(path);
            let lookup = format!("/{norm}");

            let raw = match container.get_file(&lookup) {
                Ok(d) => d,
                Err(_) => match container.get_file(path) {
                    Ok(d) => d,
                    Err(_) => continue,
                },
            };

            let html_text = decode_html(&raw);
            let cleaned = strip_scripts(&html_text);

            let title = hhc_entries
                .iter()
                .find(|e| e.local == *path)
                .map(|e| e.name.clone())
                .unwrap_or_else(|| format!("Section {}", index + 1));

            let chapter_id = format!("chm_chapter_{index:04}");
            book.add_chapter(Chapter {
                title: Some(title),
                content: cleaned,
                id: Some(chapter_id),
            });
        }

        // --- Extract image and CSS resources ---
        let resource_exts = [
            ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".svg", ".ico", ".css",
        ];
        // Clone only matching paths (not all keys) to release the borrow before get_file().
        let resource_paths: Vec<String> = container
            .content_files()
            .into_iter()
            .filter(|f| {
                let lower = f.to_ascii_lowercase();
                resource_exts.iter().any(|ext| lower.ends_with(ext))
            })
            .map(String::from)
            .collect();
        for path in &resource_paths {
            if let Ok(data) = container.get_file(path) {
                let media_type = mime_guess::from_path(path)
                    .first()
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "application/octet-stream".into());
                let norm = normalize_path(path);
                let res_id = norm.replace('/', "_");
                book.add_resource(&res_id, &norm, data, &media_type);
            }
        }

        // Fallback title
        if book.metadata.title.is_none() {
            book.metadata.title = Some("Unknown CHM Document".into());
        }

        Ok(book)
    }
}

/// Find and parse the .hhc TOC file.
fn find_and_parse_hhc(container: &mut ChmContainer, sys_info: &ChmSystemInfo) -> Vec<HhcEntry> {
    // Try path from /#SYSTEM first
    if let Some(ref hhc_path) = sys_info.contents_file {
        let paths = [
            format!("/{hhc_path}"),
            hhc_path.clone(),
            format!("/{}", normalize_path(hhc_path)),
        ];
        for p in &paths {
            if let Ok(data) = container.get_file(p) {
                let entries = parse_hhc(&data);
                if !entries.is_empty() {
                    return entries;
                }
            }
        }
    }

    // Fallback: look for any .hhc file in the directory
    let hhc_files: Vec<String> = container
        .content_files()
        .into_iter()
        .filter(|f| text_utils::ends_with_ascii_ci(f, ".hhc"))
        .map(String::from)
        .collect();

    for path in &hhc_files {
        if let Ok(data) = container.get_file(path) {
            let entries = parse_hhc(&data);
            if !entries.is_empty() {
                return entries;
            }
        }
    }

    Vec::new()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_chm() {
        let data = b"not a chm file at all";
        let mut cursor = std::io::Cursor::new(data.to_vec());
        let reader = ChmReader::new();
        assert!(reader.read_book(&mut cursor).is_err());
    }

    #[test]
    fn rejects_empty_input() {
        let mut cursor = std::io::Cursor::new(Vec::new());
        let reader = ChmReader::new();
        assert!(reader.read_book(&mut cursor).is_err());
    }

    #[test]
    fn parse_system_file_extracts_metadata() {
        // Build a synthetic /#SYSTEM file
        let mut data = Vec::new();
        // Version
        data.extend_from_slice(&3u32.to_le_bytes());
        // Entry: code=3 (title), length=10, "Test Title"
        data.extend_from_slice(&3u16.to_le_bytes());
        data.extend_from_slice(&11u16.to_le_bytes());
        data.extend_from_slice(b"Test Title\0");
        // Entry: code=2 (default topic), length=10, "index.htm"
        data.extend_from_slice(&2u16.to_le_bytes());
        data.extend_from_slice(&10u16.to_le_bytes());
        data.extend_from_slice(b"index.htm\0");

        let info = parse_system_file(&data);
        assert_eq!(info.title.as_deref(), Some("Test Title"));
        assert_eq!(info.default_topic.as_deref(), Some("index.htm"));
    }

    #[test]
    fn parse_hhc_extracts_entries() {
        let hhc = br#"
<HTML><BODY>
<UL>
<LI><OBJECT type="text/sitemap">
  <param name="Name" value="Introduction">
  <param name="Local" value="intro.htm">
</OBJECT>
<LI><OBJECT type="text/sitemap">
  <param name="Name" value="Chapter 1">
  <param name="Local" value="ch1.htm">
</OBJECT>
</UL>
</BODY></HTML>"#;
        let entries = parse_hhc(hhc);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "Introduction");
        assert_eq!(entries[0].local, "intro.htm");
        assert_eq!(entries[1].name, "Chapter 1");
        assert_eq!(entries[1].local, "ch1.htm");
    }

    #[test]
    fn strip_scripts_removes_script_tags() {
        let html = "<p>Hello</p><script>alert('x');</script><p>World</p>";
        let cleaned = strip_scripts(html);
        assert_eq!(cleaned, "<p>Hello</p><p>World</p>");
    }

    #[test]
    fn rejects_invalid_itsf_version() {
        let mut data = vec![0u8; 0x60];
        data[0..4].copy_from_slice(b"ITSF");
        data[4..8].copy_from_slice(&99i32.to_le_bytes()); // bad version
        let result = ChmContainer::parse(data);
        assert!(result.is_err());
    }

    #[test]
    fn decode_html_handles_utf8() {
        let data = "Hello, world!".as_bytes();
        assert_eq!(decode_html(data), "Hello, world!");
    }

    #[test]
    fn decode_html_handles_cp1252() {
        // 0x93 = left double quote in cp1252 → U+201C
        let data = [0x93, b'H', b'i', 0x94]; // "Hi"
        let result = decode_html(&data);
        assert!(result.contains('\u{201C}'));
        assert!(result.contains('\u{201D}'));
    }

    #[test]
    fn parse_hhc_mixed_case() {
        let hhc = br#"
<html><body>
<ul>
<li><Object Type="text/sitemap">
  <Param Name="Name" Value="Intro">
  <Param Name="Local" Value="intro.htm">
</Object>
</ul>
</body></html>"#;
        let entries = parse_hhc(hhc);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Intro");
        assert_eq!(entries[0].local, "intro.htm");
    }

    #[test]
    fn strip_scripts_case_insensitive() {
        let html = "<p>Hello</p><SCRIPT>alert('x');</SCRIPT><p>World</p>";
        let cleaned = strip_scripts(html);
        assert_eq!(cleaned, "<p>Hello</p><p>World</p>");
    }

    #[test]
    fn strip_scripts_mixed_case() {
        let html = "<p>A</p><Script type=\"text/javascript\">code();</Script><p>B</p>";
        let cleaned = strip_scripts(html);
        assert_eq!(cleaned, "<p>A</p><p>B</p>");
    }
}
