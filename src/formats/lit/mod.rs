//! LIT (Microsoft Reader) format reader and writer.
//!
//! LIT files use the ITOLITLS container: a variant of ITSS with CAOL+ITSF
//! secondary headers, IFCM/AOLL directory chunks, named sections with LZX
//! compression, and binary-encoded HTML/OPF content.

pub mod maps;
pub mod msdes;
pub mod mssha1;
pub mod unbinary;
pub mod writer;

use std::collections::HashMap;
use std::io::Read;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::domain::{Book, Chapter, FormatReader, Metadata};
use crate::error::{EruditioError, Result};
use crate::formats::common::itss::{self, DirectoryEntry};

use unbinary::{AtomTable, ManifestPath, consume_sized_utf8_string};

pub use writer::LitWriter;

const DESENCRYPT_GUID: &str = "{67F6E4A2-60BF-11D3-8540-00C04F58C3CF}";
const LZXCOMPRESS_GUID: &str = "{0A9007C6-4076-11D3-8789-0000F8105754}";

// ---------------------------------------------------------------------------
// LIT Manifest Item
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct LitManifestItem {
    internal: String,
    path: String,
    mime_type: String,
    state: String,
}

// ---------------------------------------------------------------------------
// LIT Container
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct LitContainer {
    data: Vec<u8>,
    entries: HashMap<String, DirectoryEntry>,
    content_offset: u64,
    section_names: Vec<String>,
    section_cache: HashMap<usize, Vec<u8>>,
    manifest_items: Vec<LitManifestItem>,
    book_key: Option<[u8; 8]>,
}

impl LitContainer {
    fn parse(data: Vec<u8>) -> Result<Self> {
        if data.len() < 48 {
            return Err(EruditioError::Format("LIT file too short".into()));
        }

        // --- ITOLITLS header ---
        if &data[0..8] != b"ITOLITLS" {
            return Err(EruditioError::Format(
                "Not a valid LIT file (missing ITOLITLS)".into(),
            ));
        }
        let version = itss::u32_le(&data[8..]);
        if version != 1 {
            return Err(EruditioError::Format(format!(
                "Unsupported LIT version: {version}"
            )));
        }
        // Validate i32 header fields are non-negative before casting to usize,
        // since a crafted file with negative values would wrap to huge usize values.
        let hdr_len = usize::try_from(itss::i32_le(&data[12..]))
            .map_err(|_| EruditioError::Format("Negative LIT header length".into()))?;
        let num_pieces = usize::try_from(itss::i32_le(&data[16..]))
            .map_err(|_| EruditioError::Format("Negative LIT piece count".into()))?;
        let sec_hdr_len = usize::try_from(itss::i32_le(&data[20..]))
            .map_err(|_| EruditioError::Format("Negative LIT secondary header length".into()))?;

        // --- Secondary header (CAOL + ITSF) ---
        // Use checked arithmetic to prevent overflow with crafted values.
        let sec_hdr_offset = num_pieces
            .checked_mul(16)
            .and_then(|v| v.checked_add(hdr_len))
            .ok_or_else(|| EruditioError::Format("LIT header offset overflow".into()))?;
        if sec_hdr_offset + sec_hdr_len > data.len() {
            return Err(EruditioError::Format(
                "Secondary header out of range".into(),
            ));
        }
        let sec_hdr = &data[sec_hdr_offset..sec_hdr_offset + sec_hdr_len];

        let mut entry_chunklen: u32 = 0;
        let mut entry_unknown: u32 = 0;
        let mut content_offset: u64 = 0;
        let mut found_itsf = false;

        if sec_hdr.len() >= 8 {
            let mut off = usize::try_from(itss::i32_le(&sec_hdr[4..])).map_err(|_| {
                EruditioError::Format("LIT: negative secondary header offset".into())
            })?;
            while off + 8 <= sec_hdr.len() {
                let block_type = &sec_hdr[off..off + 4];
                if block_type == b"CAOL" && off + 48 <= sec_hdr.len() {
                    entry_chunklen = itss::u32_le(&sec_hdr[off + 20..]);
                    entry_unknown = itss::u32_le(&sec_hdr[off + 28..]);
                    off += 48;
                } else if block_type == b"ITSF" && off + 20 <= sec_hdr.len() {
                    content_offset = u64::from(itss::u32_le(&sec_hdr[off + 16..]));
                    found_itsf = true;
                    off += 48.min(sec_hdr.len() - off);
                } else {
                    break;
                }
            }
        }
        if !found_itsf {
            return Err(EruditioError::Format("Missing ITSF block".into()));
        }

        // --- Read header pieces ---
        let pieces_start = hdr_len;
        let mut entries = HashMap::new();

        for i in 0..num_pieces {
            let p = pieces_start + i * 16;
            if p + 16 > data.len() {
                break;
            }
            let piece_offset = itss::u32_le(&data[p..]) as usize;
            let piece_size = usize::try_from(itss::i32_le(&data[p + 8..]))
                .map_err(|_| EruditioError::Format("LIT: negative piece size".into()))?;
            if piece_offset + piece_size > data.len() {
                continue;
            }
            let piece = &data[piece_offset..piece_offset + piece_size];

            if i == 1 {
                // Directory piece — validate and parse IFCM/AOLL
                if piece.len() < 4 || &piece[0..4] != b"IFCM" {
                    return Err(EruditioError::Format("Piece 1 is not IFCM".into()));
                }
                if piece.len() >= 28 {
                    let chunk_size = usize::try_from(itss::i32_le(&piece[8..])).map_err(|_| {
                        EruditioError::Format("LIT: negative IFCM chunk size".into())
                    })?;
                    let num_chunks = usize::try_from(itss::i32_le(&piece[24..])).map_err(|_| {
                        EruditioError::Format("LIT: negative IFCM chunk count".into())
                    })?;

                    if chunk_size > 0 && piece.len() >= 32 {
                        Self::parse_ifcm_directory(
                            piece,
                            chunk_size,
                            num_chunks,
                            entry_chunklen,
                            entry_unknown,
                            &mut entries,
                        )?;
                    }
                }
            }
        }

        // --- Read section names ---
        let section_names = Self::read_section_names(&entries, &data, content_offset)?;

        // --- Read manifest ---
        let manifest_items = Self::read_manifest(&entries, &data, content_offset)?;

        // --- DRM check ---
        // Only level-5 DRM (owner-locked license) is unrecoverable.
        // Levels 1 and 3 (DRMSealed / DRMBookplate) have keys derivable
        // from the file itself and are handled at the section transform level.
        if entries.contains_key("/DRMStorage/Licenses/EUL") {
            return Err(EruditioError::Encryption(
                "DRM-protected LIT files are not supported".into(),
            ));
        }

        // --- Derive book key for DRM levels 1 and 3 ---
        let book_key = Self::derive_book_key(&entries, &data, content_offset)?;

        Ok(LitContainer {
            data,
            entries,
            content_offset,
            section_names,
            section_cache: HashMap::new(),
            manifest_items,
            book_key,
        })
    }

    fn parse_ifcm_directory(
        piece: &[u8],
        chunk_size: usize,
        num_chunks: usize,
        _entry_chunklen: u32,
        _entry_unknown: u32,
        entries: &mut HashMap<String, DirectoryEntry>,
    ) -> Result<()> {
        for i in 0..num_chunks {
            let offset = 32 + i * chunk_size;
            if offset + chunk_size > piece.len() {
                break;
            }
            let chunk = &piece[offset..offset + chunk_size];
            if chunk.len() < 48 || &chunk[0..4] != b"AOLL" {
                continue;
            }
            let remaining_raw = usize::try_from(itss::i32_le(&chunk[4..]))
                .map_err(|_| EruditioError::Format("LIT: negative AOLL remaining value".into()))?;
            let remaining = chunk_size.saturating_sub(remaining_raw + 48);
            // Entry data starts at offset 48 within the chunk
            let entry_data = &chunk[48..];
            if let Ok(chunk_entries) = itss::parse_listing_entries(entry_data, remaining) {
                entries.extend(chunk_entries);
            }
        }
        Ok(())
    }

    /// Derive the DES book key for DRM levels 1 and 3.
    ///
    /// Returns `None` for DRM-free files. The key is computed by SHA-1 hashing
    /// `/meta` (with a 2-byte zero prefix) and `/DRMStorage/DRMSource` (and
    /// `/DRMStorage/DRMBookplate` for level 3), XOR-folding the 20-byte digest
    /// into 8 bytes, then DES-decrypting `/DRMStorage/DRMSealed` to recover the
    /// actual book key.
    fn derive_book_key(
        entries: &HashMap<String, DirectoryEntry>,
        data: &[u8],
        content_offset: u64,
    ) -> Result<Option<[u8; 8]>> {
        let drm_level = if entries.contains_key("/DRMStorage/Licenses/EUL") {
            5
        } else if entries.contains_key("/DRMStorage/DRMBookplate") {
            3
        } else if entries.contains_key("/DRMStorage/DRMSealed") {
            1
        } else {
            0
        };

        if drm_level == 0 || drm_level == 5 {
            return Ok(None);
        }

        // Hash selected files to derive the DES key
        let mut hash_names: Vec<&str> = vec!["/meta", "/DRMStorage/DRMSource"];
        if drm_level == 3 {
            hash_names.push("/DRMStorage/DRMBookplate");
        }

        let mut hasher = mssha1::MsSha1::new();
        let mut prepad = 2usize;

        for name in &hash_names {
            let entry = entries
                .get(*name)
                .ok_or_else(|| EruditioError::Parse(format!("DRM: missing {name}")))?;
            let mut file_data = Self::read_raw(data, content_offset, entry)?;

            if prepad > 0 {
                let mut padded = vec![0u8; prepad];
                padded.extend_from_slice(&file_data);
                file_data = padded;
                prepad = 0;
            }

            let postpad = 64 - (file_data.len() % 64);
            if postpad < 64 {
                file_data.resize(file_data.len() + postpad, 0);
            }

            hasher.update(&file_data);
        }

        let digest = hasher.finalize();

        let mut des_key = [0u8; 8];
        for (i, &d) in digest.iter().enumerate() {
            des_key[i % 8] ^= d;
        }

        // Decrypt the sealed entry to recover the book key
        let sealed_entry = entries
            .get("/DRMStorage/DRMSealed")
            .ok_or_else(|| EruditioError::Parse("DRM: missing DRMSealed".into()))?;
        let sealed = Self::read_raw(data, content_offset, sealed_entry)?;

        let decrypted = des_ecb_decrypt(&sealed, &des_key);

        if decrypted.is_empty() || decrypted[0] != 0 {
            return Err(EruditioError::Encryption(
                "Unable to decrypt title key".into(),
            ));
        }
        if decrypted.len() < 9 {
            return Err(EruditioError::Encryption("Decrypted key too short".into()));
        }

        let mut book_key = [0u8; 8];
        book_key.copy_from_slice(&decrypted[1..9]);
        Ok(Some(book_key))
    }

    fn read_section_names(
        entries: &HashMap<String, DirectoryEntry>,
        data: &[u8],
        content_offset: u64,
    ) -> Result<Vec<String>> {
        let entry = entries
            .get("::DataSpace/NameList")
            .ok_or_else(|| EruditioError::Parse("Missing NameList".into()))?;
        let raw = Self::read_raw(data, content_offset, entry)?;
        if raw.len() < 4 {
            return Err(EruditioError::Parse("Invalid NameList".into()));
        }
        let num_sections = itss::u16_le(&raw[2..]) as usize;
        let mut names = Vec::with_capacity(num_sections);
        let mut pos = 4;
        for _ in 0..num_sections {
            if pos + 2 > raw.len() {
                break;
            }
            let size = itss::u16_le(&raw[pos..]) as usize;
            pos += 2;
            let byte_len = size * 2 + 2; // UTF-16LE chars + null terminator
            if pos + byte_len > raw.len() {
                break;
            }
            let utf16: Vec<u16> = raw[pos..pos + byte_len]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let name = String::from_utf16_lossy(&utf16)
                .trim_end_matches('\0')
                .to_string();
            names.push(name);
            pos += byte_len;
        }
        Ok(names)
    }

    fn read_manifest(
        entries: &HashMap<String, DirectoryEntry>,
        data: &[u8],
        content_offset: u64,
    ) -> Result<Vec<LitManifestItem>> {
        let entry = match entries.get("/manifest") {
            Some(e) => e,
            None => return Ok(Vec::new()),
        };
        let raw = Self::read_raw(data, content_offset, entry)?;
        let mut items = Vec::new();
        let mut pos = 0;

        while pos < raw.len() {
            // Root string (single-byte length prefix)
            if pos >= raw.len() {
                break;
            }
            let root_len = raw[pos] as usize;
            pos += 1;
            if root_len == 0 {
                break;
            }
            if pos + root_len > raw.len() {
                break;
            }
            let _root = String::from_utf8_lossy(&raw[pos..pos + root_len]).to_string();
            pos += root_len;

            for state_name in &["spine", "not spine", "css", "images"] {
                if pos + 4 > raw.len() {
                    break;
                }
                let num_files = usize::try_from(itss::i32_le(&raw[pos..])).map_err(|_| {
                    EruditioError::Format("LIT: negative manifest file count".into())
                })?;
                pos += 4;
                for _ in 0..num_files {
                    if pos + 4 > raw.len() {
                        break;
                    }
                    let _offset = itss::u32_le(&raw[pos..]);
                    pos += 4;
                    let (internal, new_pos) = consume_sized_utf8_string(&raw, pos, false)?;
                    pos = new_pos;
                    let (original, new_pos) = consume_sized_utf8_string(&raw, pos, false)?;
                    pos = new_pos;
                    let (mime_type, new_pos) = consume_sized_utf8_string(&raw, pos, true)?;
                    pos = new_pos;

                    let path = normalize_lit_path(&original);
                    items.push(LitManifestItem {
                        internal,
                        path,
                        mime_type: mime_type.to_lowercase(),
                        state: state_name.to_string(),
                    });
                }
            }
        }

        // Strip common path prefix
        strip_common_prefix(&mut items);
        Ok(items)
    }

    fn read_raw(data: &[u8], content_offset: u64, entry: &DirectoryEntry) -> Result<Vec<u8>> {
        // Use checked arithmetic to prevent overflow from crafted offsets/sizes.
        let start = content_offset
            .checked_add(entry.offset)
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| EruditioError::Parse("LIT entry offset overflow".into()))?;
        let end = start
            .checked_add(entry.size as usize)
            .ok_or_else(|| EruditioError::Parse("LIT entry size overflow".into()))?;
        if end > data.len() {
            return Err(EruditioError::Parse("Entry extends past file end".into()));
        }
        Ok(data[start..end].to_vec())
    }

    fn get_file(&mut self, name: &str) -> Result<Vec<u8>> {
        let entry = self
            .entries
            .get(name)
            .ok_or_else(|| EruditioError::Parse(format!("LIT entry not found: {name}")))?
            .clone();
        if entry.section == 0 {
            Self::read_raw(&self.data, self.content_offset, &entry)
        } else {
            let section = entry.section as usize;
            if !self.section_cache.contains_key(&section) {
                let decompressed = self.decompress_section(section)?;
                self.section_cache.insert(section, decompressed);
            }
            let section_data = &self.section_cache[&section];
            let start = entry.offset as usize;
            let end = (start + entry.size as usize).min(section_data.len());
            if start > section_data.len() {
                return Err(EruditioError::Parse(format!(
                    "Entry '{name}' offset past section data"
                )));
            }
            Ok(section_data[start..end].to_vec())
        }
    }

    fn decompress_section(&self, section: usize) -> Result<Vec<u8>> {
        if section >= self.section_names.len() {
            return Err(EruditioError::Parse(format!(
                "Section {section} out of range"
            )));
        }
        let name = &self.section_names[section];
        let path = format!("::DataSpace/Storage/{name}");

        let transform_entry = self.entries.get(&format!("{path}/Transform/List"));
        let content_entry = self
            .entries
            .get(&format!("{path}/Content"))
            .ok_or_else(|| EruditioError::Parse(format!("Missing {path}/Content")))?;
        let control_entry = self
            .entries
            .get(&format!("{path}/ControlData"))
            .ok_or_else(|| EruditioError::Parse(format!("Missing {path}/ControlData")))?;

        let mut content = Self::read_raw(&self.data, self.content_offset, content_entry)?;
        let mut control = Self::read_raw(&self.data, self.content_offset, control_entry)?;

        let transform = match transform_entry {
            Some(e) => Self::read_raw(&self.data, self.content_offset, e)?,
            None => return Ok(content),
        };

        let mut t_pos = 0;
        while t_pos + 16 <= transform.len() {
            let guid = itss::format_guid(&transform[t_pos..])?;
            // Compute control block size with checked arithmetic to avoid
            // signed overflow when the i32 field contains crafted values.
            let csize = if control.len() >= 4 {
                let raw = itss::i32_le(&control);
                raw.checked_add(1)
                    .and_then(|v| v.checked_mul(4))
                    .and_then(|v| usize::try_from(v).ok())
                    .unwrap_or(0)
            } else {
                0
            };

            if guid == DESENCRYPT_GUID {
                if let Some(ref key) = self.book_key {
                    content = des_ecb_decrypt(&content, key);
                } else {
                    return Err(EruditioError::Encryption(
                        "DRM-protected LIT file (no book key)".into(),
                    ));
                }
                if csize <= control.len() {
                    control = control[csize..].to_vec();
                }
            } else if guid == LZXCOMPRESS_GUID {
                let rt_path = format!(
                    "::DataSpace/Storage/{name}/Transform/{LZXCOMPRESS_GUID}/InstanceData/ResetTable"
                );
                let rt_entry = self.entries.get(&rt_path).ok_or_else(|| {
                    EruditioError::Parse(format!("Missing reset table: {rt_path}"))
                })?;
                let reset_table = Self::read_raw(&self.data, self.content_offset, rt_entry)?;
                content = lit_lzx_decompress(&content, &control, &reset_table)?;
                if csize <= control.len() {
                    control = control[csize..].to_vec();
                }
            } else {
                return Err(EruditioError::Format(format!(
                    "Unknown LIT transform: {guid}"
                )));
            }
            t_pos += 16;
        }

        Ok(content)
    }

    fn get_atoms(&mut self, internal: &str) -> Result<AtomTable> {
        let atom_path = format!("/data/{internal}/atom");
        let raw = match self.get_file(&atom_path) {
            Ok(d) => d,
            Err(_) => return Ok(AtomTable::default()),
        };
        if raw.len() < 4 {
            return Ok(AtomTable::default());
        }
        let mut tags = HashMap::new();
        let mut pos = 0;
        let ntags = itss::u32_le(&raw[pos..]) as usize;
        pos += 4;
        for i in 1..=ntags {
            if pos >= raw.len() {
                break;
            }
            let size = raw[pos] as usize;
            pos += 1;
            if size == 0 || pos + size > raw.len() {
                break;
            }
            tags.insert(
                i as u32,
                String::from_utf8_lossy(&raw[pos..pos + size]).to_string(),
            );
            pos += size;
        }

        let mut attrs = HashMap::new();
        if pos + 4 <= raw.len() {
            let nattrs = itss::u32_le(&raw[pos..]) as usize;
            pos += 4;
            for i in 1..=nattrs {
                if pos + 4 > raw.len() {
                    break;
                }
                let size = itss::u32_le(&raw[pos..]) as usize;
                pos += 4;
                if size == 0 || pos + size > raw.len() {
                    break;
                }
                attrs.insert(
                    i as u32,
                    String::from_utf8_lossy(&raw[pos..pos + size]).to_string(),
                );
                pos += size;
            }
        }

        Ok(AtomTable { tags, attrs })
    }

    fn manifest_paths(&self) -> HashMap<String, ManifestPath> {
        self.manifest_items
            .iter()
            .map(|item| {
                (
                    item.internal.clone(),
                    ManifestPath {
                        path: item.path.clone(),
                    },
                )
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// DES ECB decryption (for DRM levels 1 and 3)
// ---------------------------------------------------------------------------

fn des_ecb_decrypt(data: &[u8], key: &[u8; 8]) -> Vec<u8> {
    let cipher = msdes::MsDes::new_decrypt(key);
    cipher.decrypt_ecb(data)
}

// ---------------------------------------------------------------------------
// LZX decompression (LIT variant)
// ---------------------------------------------------------------------------

fn lit_lzx_decompress(content: &[u8], control: &[u8], reset_table: &[u8]) -> Result<Vec<u8>> {
    let window_size_bytes = itss::parse_lzxc_control_data_lit(control)?;
    let window_size = itss::window_size_from_bytes(window_size_bytes)?;

    // Parse the LIT reset table into block addresses. The physical layout is
    // identical to CHM reset tables but calibre never checks the version field,
    // so we skip that validation here.
    let rt = parse_lit_reset_table(reset_table)?;

    // Reuse the proven block-by-block CHM decompressor — the LZX stream format
    // is the same; only the container framing differs.
    itss::lzx_decompress_section(content, window_size, &rt)
}

/// Parse a LIT-style LZXC reset table into the shared [`itss::LzxResetTable`].
///
/// Layout (all LE, same as CHM):
/// - offset  4: block_count (u32)
/// - offset 12: table_offset (u32) — byte offset to block address array
/// - offset 16: uncompressed_len (u64)
/// - offset 24: compressed_len (u64)
/// - offset 32: block_len (u64)
/// - at table_offset: `block_count + 1` u64 entries (compressed byte offsets)
fn parse_lit_reset_table(data: &[u8]) -> Result<itss::LzxResetTable> {
    if data.len() < 40 {
        return Err(EruditioError::Parse("LIT reset table too short".into()));
    }

    let block_count = itss::u32_le(&data[4..]);
    let table_offset = itss::u32_le(&data[12..]) as usize;
    let uncompressed_len = itss::u64_le(&data[16..]);
    let compressed_len = itss::u64_le(&data[24..]);
    let block_len = itss::u64_le(&data[32..]);

    if block_len == 0 {
        return Err(EruditioError::Compression(
            "LIT reset table block_len is zero".into(),
        ));
    }

    const MAX_RESET_TABLE_ENTRIES: usize = 16_000_000;

    let num_entries = (block_count as usize)
        .checked_add(1)
        .ok_or_else(|| EruditioError::Parse("LIT reset table entry count overflow".into()))?;
    if num_entries > MAX_RESET_TABLE_ENTRIES {
        return Err(EruditioError::Parse(
            "LIT reset table has too many entries (possible corrupt file)".into(),
        ));
    }
    let mut block_addresses = Vec::with_capacity(num_entries);
    for i in 0..num_entries {
        let off = table_offset + i * 8;
        if off + 8 > data.len() {
            break;
        }
        block_addresses.push(itss::u64_le(&data[off..]));
    }

    Ok(itss::LzxResetTable {
        block_count,
        block_len,
        uncompressed_len,
        compressed_len,
        block_addresses,
    })
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn normalize_lit_path(original: &str) -> String {
    let mut path = original.replace('\\', "/");
    // Strip drive letter (e.g., "C:/")
    if path.len() >= 3 && path.as_bytes()[1] == b':' && path.as_bytes()[2] == b'/' {
        path = path[2..].to_string();
    }
    // Strip leading "../"
    while path.starts_with("../") {
        path = path[3..].to_string();
    }
    // Strip leading "/"
    path = path.trim_start_matches('/').to_string();
    path
}

fn strip_common_prefix(items: &mut [LitManifestItem]) {
    if items.len() <= 1 {
        return;
    }
    // Start with the directory portion of the first path
    let mut shared = match items[0].path.rfind('/') {
        Some(idx) => items[0].path[..idx + 1].to_string(),
        None => return, // No directory component — nothing to strip
    };
    for item in items.iter().skip(1) {
        while !shared.is_empty() && !item.path.starts_with(&shared) {
            // Go up one directory level: strip trailing '/' then find parent
            let trimmed = shared.trim_end_matches('/');
            match trimmed.rfind('/') {
                Some(idx) => shared = trimmed[..idx + 1].to_string(),
                None => {
                    shared.clear();
                },
            }
        }
        if shared.is_empty() {
            break;
        }
    }
    if !shared.is_empty() {
        let prefix_len = shared.len();
        for item in items.iter_mut() {
            if item.path.len() > prefix_len {
                item.path = item.path[prefix_len..].to_string();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// OPF metadata parsing
// ---------------------------------------------------------------------------

fn parse_opf_metadata(opf_xml: &str, metadata: &mut Metadata) {
    let mut reader = Reader::from_str(opf_xml);
    let mut current_tag = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                current_tag = String::from_utf8_lossy(e.name().as_ref()).to_lowercase();
            },
            Ok(Event::Text(e)) => {
                let text = String::from_utf8_lossy(&e.into_inner()).trim().to_string();
                if !text.is_empty() {
                    match current_tag.as_str() {
                        "dc:title" if metadata.title.is_none() => {
                            metadata.title = Some(text);
                        },
                        "dc:creator" => {
                            metadata.authors.push(text);
                        },
                        "dc:language" if metadata.language.is_none() => {
                            metadata.language = Some(text);
                        },
                        "dc:publisher" if metadata.publisher.is_none() => {
                            metadata.publisher = Some(text);
                        },
                        "dc:description" if metadata.description.is_none() => {
                            metadata.description = Some(text);
                        },
                        "dc:identifier" if metadata.identifier.is_none() => {
                            metadata.identifier = Some(text);
                        },
                        "dc:subject" => {
                            metadata.subjects.push(text);
                        },
                        "dc:rights" if metadata.rights.is_none() => {
                            metadata.rights = Some(text);
                        },
                        _ => {},
                    }
                }
                current_tag.clear();
            },
            Ok(Event::End(_)) => {
                current_tag.clear();
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {},
        }
        buf.clear();
    }
}

// ---------------------------------------------------------------------------
// LitReader
// ---------------------------------------------------------------------------

/// LIT (Microsoft Reader) format reader.
#[derive(Default)]
pub struct LitReader;

impl LitReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for LitReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).map_err(EruditioError::Io)?;

        let mut container = LitContainer::parse(buffer)?;
        let mut book = Book::new();

        // --- OPF metadata ---
        if let Ok(meta_raw) = container.get_file("/meta") {
            let manifest_paths = container.manifest_paths();
            if let Ok(opf_xml) = unbinary::unbinary_to_html(
                &meta_raw,
                "content.opf",
                &manifest_paths,
                &maps::OPF_MAP,
                &AtomTable::default(),
            ) {
                parse_opf_metadata(&opf_xml, &mut book.metadata);
            }
        }

        // --- Spine items (chapters) ---
        let spine_items: Vec<LitManifestItem> = container
            .manifest_items
            .iter()
            .filter(|i| i.state == "spine")
            .cloned()
            .collect();

        let manifest_paths = container.manifest_paths();
        for (idx, item) in spine_items.iter().enumerate() {
            let content_path = format!("/data/{}/content", item.internal);
            let raw = match container.get_file(&content_path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let atoms = container.get_atoms(&item.internal)?;
            let html = unbinary::unbinary_to_html(
                &raw,
                &item.path,
                &manifest_paths,
                &maps::HTML_MAP,
                &atoms,
            )?;

            let chapter_id = format!("lit_ch_{idx:04}");
            book.add_chapter(&Chapter {
                title: None,
                content: html,
                id: Some(chapter_id),
            });
        }

        // --- Non-spine resources ---
        let resource_items: Vec<LitManifestItem> = container
            .manifest_items
            .iter()
            .filter(|i| i.state != "spine")
            .cloned()
            .collect();

        for item in &resource_items {
            let data_path = format!("/data/{}", item.internal);
            if let Ok(data) = container.get_file(&data_path) {
                let media = if item.mime_type.is_empty() {
                    mime_guess::from_path(&item.path)
                        .first()
                        .map(|m| m.to_string())
                        .unwrap_or_else(|| "application/octet-stream".into())
                } else {
                    item.mime_type.clone()
                };
                book.add_resource(&item.internal, &item.path, data, &media);
            }
        }

        if book.metadata.title.is_none() {
            book.metadata.title = Some("Unknown LIT Document".into());
        }

        Ok(book)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_lit() {
        let data = b"not a lit file at all";
        let mut cursor = std::io::Cursor::new(data.to_vec());
        let reader = LitReader::new();
        assert!(reader.read_book(&mut cursor).is_err());
    }

    #[test]
    fn rejects_empty_input() {
        let mut cursor = std::io::Cursor::new(Vec::new());
        let reader = LitReader::new();
        assert!(reader.read_book(&mut cursor).is_err());
    }

    #[test]
    fn rejects_wrong_version() {
        let mut data = vec![0u8; 64];
        data[0..8].copy_from_slice(b"ITOLITLS");
        data[8..12].copy_from_slice(&99u32.to_le_bytes());
        let result = LitContainer::parse(data);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("version"));
    }

    #[test]
    fn normalize_lit_path_handles_backslash() {
        assert_eq!(normalize_lit_path("content\\ch1.html"), "content/ch1.html");
    }

    #[test]
    fn normalize_lit_path_strips_drive() {
        assert_eq!(normalize_lit_path("C:/docs/ch1.html"), "docs/ch1.html");
    }

    #[test]
    fn normalize_lit_path_strips_dotdot() {
        assert_eq!(normalize_lit_path("../../ch1.html"), "ch1.html");
    }

    #[test]
    fn parse_opf_extracts_title_and_author() {
        let opf = r#"<package>
            <metadata><dc-metadata>
                <dc:Title>Test Book</dc:Title>
                <dc:Creator>Jane Doe</dc:Creator>
                <dc:Language>en</dc:Language>
            </dc-metadata></metadata>
        </package>"#;
        let mut metadata = Metadata::default();
        parse_opf_metadata(opf, &mut metadata);
        assert_eq!(metadata.title.as_deref(), Some("Test Book"));
        assert_eq!(metadata.authors, vec!["Jane Doe"]);
        assert_eq!(metadata.language.as_deref(), Some("en"));
    }

    #[test]
    fn strip_common_prefix_basic() {
        let mut items = vec![
            LitManifestItem {
                internal: "a".into(),
                path: "content/ch1.html".into(),
                mime_type: "text/html".into(),
                state: "spine".into(),
            },
            LitManifestItem {
                internal: "b".into(),
                path: "content/ch2.html".into(),
                mime_type: "text/html".into(),
                state: "spine".into(),
            },
        ];
        strip_common_prefix(&mut items);
        assert_eq!(items[0].path, "ch1.html");
        assert_eq!(items[1].path, "ch2.html");
    }

    #[test]
    fn atom_table_defaults_empty() {
        let atoms = AtomTable::default();
        assert!(atoms.tags.is_empty());
        assert!(atoms.attrs.is_empty());
    }
}
