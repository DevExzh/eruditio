//! Shared ITSS container primitives for CHM and LIT formats.
//!
//! Both CHM (Compiled HTML Help) and LIT (Microsoft Reader) use variants of the
//! ITSS (Information Transform Storage Set) container format. This module provides
//! shared primitives: variable-length integer encoding, directory entry parsing,
//! LZX reset table parsing, and LZX section decompression.

use std::collections::HashMap;

use lzxd::{Lzxd, WindowSize};

use crate::error::{EruditioError, Result};

// ---------------------------------------------------------------------------
// Little-endian byte reading helpers
// ---------------------------------------------------------------------------

/// Read a little-endian u16 from a byte slice (panics if < 2 bytes).
pub fn u16_le(data: &[u8]) -> u16 {
    u16::from_le_bytes([data[0], data[1]])
}

/// Read a little-endian u32 from a byte slice (panics if < 4 bytes).
pub fn u32_le(data: &[u8]) -> u32 {
    u32::from_le_bytes([data[0], data[1], data[2], data[3]])
}

/// Read a little-endian i32 from a byte slice (panics if < 4 bytes).
pub fn i32_le(data: &[u8]) -> i32 {
    i32::from_le_bytes([data[0], data[1], data[2], data[3]])
}

/// Read a little-endian u64 from a byte slice (panics if < 8 bytes).
pub fn u64_le(data: &[u8]) -> u64 {
    u64::from_le_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ])
}

// ---------------------------------------------------------------------------
// GUID formatting (for LIT transform identification)
// ---------------------------------------------------------------------------

/// Format a 16-byte GUID in Microsoft registry format: `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`.
///
/// Returns an error if `data` contains fewer than 16 bytes.
pub fn format_guid(data: &[u8]) -> Result<String> {
    if data.len() < 16 {
        return Err(EruditioError::Parse(
            "GUID data too short (need 16 bytes)".into(),
        ));
    }
    let d1 = u32_le(&data[0..4]);
    let d2 = u16_le(&data[4..6]);
    let d3 = u16_le(&data[6..8]);
    Ok(format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        d1, d2, d3, data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
    ))
}

// ---------------------------------------------------------------------------
// Variable-length integer (encint)
// ---------------------------------------------------------------------------

/// Decode a variable-length integer from `data`.
///
/// Each byte contributes 7 bits of value; the high bit signals continuation.
/// Returns `(value, bytes_consumed)`.
pub fn encint(data: &[u8]) -> Result<(u64, usize)> {
    let mut val: u64 = 0;
    let mut pos = 0;
    loop {
        if pos >= data.len() {
            return Err(EruditioError::Parse(
                "Unexpected end of data in encint".into(),
            ));
        }
        // A u64 holds at most 64 bits; each continuation byte contributes 7,
        // so more than 10 bytes would overflow.
        if pos >= 10 {
            return Err(EruditioError::Parse("encint overflow (>10 bytes)".into()));
        }
        let b = data[pos];
        pos += 1;
        val = val
            .checked_shl(7)
            .and_then(|v| v.checked_add(u64::from(b & 0x7F)))
            .ok_or_else(|| EruditioError::Parse("encint value overflow".into()))?;
        if b & 0x80 == 0 {
            break;
        }
    }
    Ok((val, pos))
}

// ---------------------------------------------------------------------------
// Directory entry
// ---------------------------------------------------------------------------

/// A single entry in an ITSS directory listing (shared by PMGL/AOLL chunks).
#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    pub name: String,
    pub section: u32,
    pub offset: u64,
    pub size: u64,
}

/// Parse directory entries from raw listing data.
///
/// Both CHM (PMGL) and LIT (AOLL) listing chunks encode entries identically:
/// `encint(name_len) | name_bytes | encint(section) | encint(offset) | encint(size)`
///
/// `data_len` is the number of usable bytes in `data` (may be less than `data.len()`
/// due to free space at the end of a chunk).
pub fn parse_listing_entries(
    data: &[u8],
    data_len: usize,
) -> Result<HashMap<String, DirectoryEntry>> {
    let mut entries = HashMap::new();
    let mut pos = 0;
    let end = data_len.min(data.len());

    while pos < end {
        // Name length
        let remaining = &data[pos..end];
        if remaining.is_empty() {
            break;
        }
        let (name_len, consumed) = encint(remaining)?;
        pos += consumed;
        let name_len = name_len as usize;

        // Name bytes (UTF-8)
        if pos + name_len > end {
            break;
        }
        let name = match std::str::from_utf8(&data[pos..pos + name_len]) {
            Ok(s) => s.to_string(),
            Err(_) => break,
        };
        pos += name_len;

        // Section
        let remaining = &data[pos..end];
        if remaining.is_empty() {
            break;
        }
        let (section, consumed) = encint(remaining)?;
        pos += consumed;

        // Offset
        let remaining = &data[pos..end];
        if remaining.is_empty() {
            break;
        }
        let (offset, consumed) = encint(remaining)?;
        pos += consumed;

        // Size
        let remaining = &data[pos..end];
        if remaining.is_empty() {
            break;
        }
        let (size, consumed) = encint(remaining)?;
        pos += consumed;

        entries.insert(
            name.clone(),
            DirectoryEntry {
                name,
                section: u32::try_from(section).unwrap_or(u32::MAX),
                offset,
                size,
            },
        );
    }

    Ok(entries)
}

// ---------------------------------------------------------------------------
// WindowSize conversion
// ---------------------------------------------------------------------------

/// Convert a window size in bytes to the `lzxd` crate's `WindowSize` enum.
pub fn window_size_from_bytes(bytes: u32) -> Result<WindowSize> {
    match bytes {
        0x0000_8000 => Ok(WindowSize::KB32),
        0x0001_0000 => Ok(WindowSize::KB64),
        0x0002_0000 => Ok(WindowSize::KB128),
        0x0004_0000 => Ok(WindowSize::KB256),
        0x0008_0000 => Ok(WindowSize::KB512),
        0x0010_0000 => Ok(WindowSize::MB1),
        0x0020_0000 => Ok(WindowSize::MB2),
        0x0040_0000 => Ok(WindowSize::MB4),
        0x0080_0000 => Ok(WindowSize::MB8),
        0x0100_0000 => Ok(WindowSize::MB16),
        0x0200_0000 => Ok(WindowSize::MB32),
        _ => Err(EruditioError::Compression(format!(
            "Unsupported LZX window size: {bytes}"
        ))),
    }
}

/// Convert a window-size bit count (e.g. 15 = 32KB, 16 = 64KB) to `WindowSize`.
pub fn window_size_from_bits(bits: u32) -> Result<WindowSize> {
    if !(15..=21).contains(&bits) {
        return Err(EruditioError::Compression(format!(
            "Invalid LZX window size bits: {bits}"
        )));
    }
    window_size_from_bytes(1u32 << bits)
}

// ---------------------------------------------------------------------------
// LZX Reset Table
// ---------------------------------------------------------------------------

/// Parsed LZXC reset table — provides per-block compressed byte offsets.
#[derive(Debug)]
pub struct LzxResetTable {
    /// Number of blocks (each block produces `block_len` uncompressed bytes).
    pub block_count: u32,
    /// Uncompressed bytes per block (typically 32768).
    pub block_len: u64,
    /// Total uncompressed length of the section.
    pub uncompressed_len: u64,
    /// Total compressed length of the section.
    pub compressed_len: u64,
    /// Compressed byte offset for each block. Length = `block_count + 1`
    /// (the extra entry marks the end of the last block).
    pub block_addresses: Vec<u64>,
}

/// Parse an LZXC reset table from raw bytes.
///
/// Layout (all LE):
/// - offset  0: version (u32, must be 2)
/// - offset  4: block_count (u32)
/// - offset  8: unknown (u32)
/// - offset 12: table_offset (u32) — byte offset from start to block address array
/// - offset 16: uncompressed_len (u64)
/// - offset 24: compressed_len (u64)
/// - offset 32: block_len (u64)
/// - at table_offset: `block_count + 1` u64 entries (compressed byte offsets)
pub fn parse_lzx_reset_table(data: &[u8]) -> Result<LzxResetTable> {
    if data.len() < 40 {
        return Err(EruditioError::Parse("Reset table too short".into()));
    }

    let version = u32_le(&data[0..]);
    if version != 2 {
        return Err(EruditioError::Parse(format!(
            "Unsupported reset table version: {version}"
        )));
    }

    let block_count = u32_le(&data[4..]);
    let table_offset = u32_le(&data[12..]) as usize;
    let uncompressed_len = u64_le(&data[16..]);
    let compressed_len = u64_le(&data[24..]);
    let block_len = u64_le(&data[32..]);

    // Read block address entries
    let num_entries = (block_count as usize)
        .checked_add(1)
        .ok_or_else(|| EruditioError::Parse("reset table block_count overflow".into()))?;
    let entry_end = table_offset
        .checked_add(
            num_entries
                .checked_mul(8)
                .ok_or_else(|| EruditioError::Parse("reset table entry size overflow".into()))?,
        )
        .ok_or_else(|| EruditioError::Parse("reset table entry offset overflow".into()))?;
    if entry_end > data.len() {
        // Fall back: read as many entries as available
        let available = (data.len().saturating_sub(table_offset)) / 8;
        let mut block_addresses = Vec::with_capacity(available);
        for i in 0..available {
            let off = table_offset + i * 8;
            block_addresses.push(u64_le(&data[off..]));
        }
        return Ok(LzxResetTable {
            block_count,
            block_len,
            uncompressed_len,
            compressed_len,
            block_addresses,
        });
    }

    let mut block_addresses = Vec::with_capacity(num_entries);
    for i in 0..num_entries {
        let off = table_offset + i * 8;
        block_addresses.push(u64_le(&data[off..]));
    }

    Ok(LzxResetTable {
        block_count,
        block_len,
        uncompressed_len,
        compressed_len,
        block_addresses,
    })
}

// ---------------------------------------------------------------------------
// LZX Section Decompression
// ---------------------------------------------------------------------------

/// Decompress an LZX-compressed section using the `lzxd` crate.
///
/// The `content` slice is the raw compressed data. `window_size` determines the
/// LZX window and reset interval. The `reset_table` provides per-block compressed
/// byte offsets so each 32 KB block can be independently located.
///
/// The LZX decoder state is reset at window-size boundaries (every
/// `window_size / block_len` blocks).
pub fn lzx_decompress_section(
    content: &[u8],
    window_size: WindowSize,
    reset_table: &LzxResetTable,
) -> Result<Vec<u8>> {
    let block_len = reset_table.block_len;
    if block_len == 0 {
        return Err(EruditioError::Compression(
            "Reset table block_len is zero".into(),
        ));
    }

    let window_bytes = window_size as u64;
    let blocks_per_reset = (window_bytes / block_len).max(1) as usize;
    let total_blocks = reset_table.block_count as usize;
    let uncompressed_len = reset_table.uncompressed_len;

    // Cap pre-allocation to prevent OOM from crafted headers claiming huge sizes.
    // The actual output grows as blocks are decompressed, so a smaller initial
    // capacity just means a few extra reallocations for legitimate large files.
    const MAX_PREALLOC: usize = 64 * 1024 * 1024; // 64 MB
    let mut result = Vec::with_capacity((uncompressed_len as usize).min(MAX_PREALLOC));
    let mut decoder: Option<Lzxd> = None;

    for i in 0..total_blocks {
        // Reset at window-size boundaries
        if i % blocks_per_reset == 0 {
            decoder = Some(Lzxd::new(window_size));
        }

        let lzx = decoder
            .as_mut()
            .ok_or_else(|| EruditioError::Compression("LZX decoder not initialized".into()))?;

        // Determine compressed data range for this block
        let comp_start = if i < reset_table.block_addresses.len() {
            reset_table.block_addresses[i] as usize
        } else {
            content.len()
        };
        let comp_end = if i + 1 < reset_table.block_addresses.len() {
            reset_table.block_addresses[i + 1] as usize
        } else {
            content.len()
        };

        let comp_start = comp_start.min(content.len());
        let comp_end = comp_end.min(content.len());
        if comp_start >= comp_end {
            break;
        }
        let compressed_block = &content[comp_start..comp_end];

        // Output size for this block
        let bytes_produced = (i as u64) * block_len;
        let output_size = (block_len.min(uncompressed_len.saturating_sub(bytes_produced))) as usize;
        if output_size == 0 {
            break;
        }

        match lzx.decompress_next(compressed_block, output_size) {
            Ok(decompressed) => {
                result.extend_from_slice(decompressed);
            },
            Err(e) => {
                // Log warning but try to continue with partial data
                log::warn!("LZX decompression error at block {i}: {e}");
                break;
            },
        }
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// LZXC Control Data parsing
// ---------------------------------------------------------------------------

/// Parsed LZXC control data block.
#[derive(Debug)]
pub struct LzxcControlData {
    /// LZX window size in bytes.
    pub window_size: u32,
    /// Reset interval in bytes (how often the decoder state resets).
    pub reset_interval: u32,
}

/// Parse CHM-style LZXC control data.
///
/// Layout (all LE):
/// - offset  0: size (u32)
/// - offset  4: signature (4 bytes, must be "LZXC")
/// - offset  8: version (u32, 1 or 2)
/// - offset 12: reset_interval (u32)
/// - offset 16: window_size (u32)
/// - offset 20: windows_per_reset (u32)
///
/// For version 2, `reset_interval` and `window_size` are multiplied by 0x8000.
pub fn parse_lzxc_control_data_chm(data: &[u8]) -> Result<LzxcControlData> {
    if data.len() < 24 {
        return Err(EruditioError::Parse("LZXC control data too short".into()));
    }
    if &data[4..8] != b"LZXC" {
        return Err(EruditioError::Parse("Invalid LZXC signature".into()));
    }

    let version = u32_le(&data[8..]);
    let mut reset_interval = u32_le(&data[12..]);
    let mut window_size = u32_le(&data[16..]);

    if version == 2 {
        reset_interval = reset_interval.saturating_mul(0x8000);
        window_size = window_size.saturating_mul(0x8000);
    } else if version != 1 {
        return Err(EruditioError::Parse(format!(
            "Unknown LZXC version: {version}"
        )));
    }

    if window_size == 0 || reset_interval == 0 {
        return Err(EruditioError::Parse(
            "LZXC control data has zero window or reset interval".into(),
        ));
    }

    Ok(LzxcControlData {
        window_size,
        reset_interval,
    })
}

/// Parse LIT-style LZXC control data.
///
/// LIT stores the window size parameter at offset 12 of the control data.
/// The value is a power-of-2 indicator: window_bits = 14 + floor(log2(value + 1)).
pub fn parse_lzxc_control_data_lit(data: &[u8]) -> Result<u32> {
    if data.len() < 32 {
        return Err(EruditioError::Parse("LIT control data too short".into()));
    }
    if &data[4..8] != b"LZXC" {
        return Err(EruditioError::Parse("Invalid LZXC signature in LIT".into()));
    }

    let raw = u32_le(&data[12..]);
    let mut window_bits: u32 = 14;
    let mut u = raw;
    while u > 0 {
        u >>= 1;
        window_bits += 1;
    }

    if !(15..=25).contains(&window_bits) {
        return Err(EruditioError::Compression(format!(
            "Invalid LIT window size bits: {window_bits}"
        )));
    }

    Ok(1u32 << window_bits)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encint_single_byte() {
        let data = [0x42];
        let (val, consumed) = encint(&data).unwrap();
        assert_eq!(val, 0x42);
        assert_eq!(consumed, 1);
    }

    #[test]
    fn encint_two_bytes() {
        // 0x81 0x00 → (1 << 7) | 0 = 128
        let data = [0x81, 0x00];
        let (val, consumed) = encint(&data).unwrap();
        assert_eq!(val, 128);
        assert_eq!(consumed, 2);
    }

    #[test]
    fn encint_three_bytes() {
        // 0x81 0x80 0x00 → ((1 << 7) | 0) << 7 | 0 = 128 << 7 = 16384
        let data = [0x81, 0x80, 0x00];
        let (val, consumed) = encint(&data).unwrap();
        assert_eq!(val, 16384);
        assert_eq!(consumed, 3);
    }

    #[test]
    fn encint_truncated() {
        let data = [0x81]; // continuation bit set but no more bytes
        assert!(encint(&data).is_err());
    }

    #[test]
    fn encint_zero() {
        let data = [0x00];
        let (val, consumed) = encint(&data).unwrap();
        assert_eq!(val, 0);
        assert_eq!(consumed, 1);
    }

    #[test]
    fn parse_listing_entries_basic() {
        // Build a synthetic listing entry:
        // name_len=4, name="test", section=0, offset=100, size=50
        let mut data = Vec::new();
        data.push(4); // name_len (encint, single byte)
        data.extend_from_slice(b"test"); // name
        data.push(0); // section (encint)
        data.push(100); // offset (encint)
        data.push(50); // size (encint)

        let entries = parse_listing_entries(&data, data.len()).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries["test"];
        assert_eq!(entry.name, "test");
        assert_eq!(entry.section, 0);
        assert_eq!(entry.offset, 100);
        assert_eq!(entry.size, 50);
    }

    #[test]
    fn parse_listing_entries_multiple() {
        let mut data = Vec::new();
        // Entry 1: "/foo"
        data.push(4);
        data.extend_from_slice(b"/foo");
        data.push(0); // section
        data.push(0); // offset
        data.push(100); // size
        // Entry 2: "/bar" — all values <= 127 to stay single-byte encint
        data.push(4);
        data.extend_from_slice(b"/bar");
        data.push(1); // section
        data.push(50); // offset
        data.push(120); // size (must be <= 127 for single-byte encint)

        let entries = parse_listing_entries(&data, data.len()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries["/foo"].section, 0);
        assert_eq!(entries["/bar"].section, 1);
        assert_eq!(entries["/bar"].size, 120);
    }

    #[test]
    fn window_size_conversions() {
        assert_eq!(window_size_from_bytes(0x8000).unwrap(), WindowSize::KB32);
        assert_eq!(window_size_from_bytes(0x10000).unwrap(), WindowSize::KB64);
        assert!(window_size_from_bytes(12345).is_err());

        assert_eq!(window_size_from_bits(15).unwrap(), WindowSize::KB32);
        assert_eq!(window_size_from_bits(16).unwrap(), WindowSize::KB64);
        assert!(window_size_from_bits(14).is_err());
        assert!(window_size_from_bits(26).is_err());
    }

    #[test]
    fn format_guid_basic() {
        // {67F6E4A2-60BF-11D3-8540-00C04F58C3CF}
        let data: [u8; 16] = [
            0xA2, 0xE4, 0xF6, 0x67, // d1 LE
            0xBF, 0x60, // d2 LE
            0xD3, 0x11, // d3 LE
            0x85, 0x40, 0x00, 0xC0, 0x4F, 0x58, 0xC3, 0xCF,
        ];
        assert_eq!(
            format_guid(&data).unwrap(),
            "{67F6E4A2-60BF-11D3-8540-00C04F58C3CF}"
        );
    }

    #[test]
    fn parse_reset_table_basic() {
        // Build a minimal reset table: version=2, block_count=2, unknown=0,
        // table_offset=40, uncompressed_len=65536, compressed_len=40000,
        // block_len=32768, then 3 entries (0, 20000, 40000)
        let mut data = vec![0u8; 40 + 3 * 8];
        data[0..4].copy_from_slice(&2u32.to_le_bytes()); // version
        data[4..8].copy_from_slice(&2u32.to_le_bytes()); // block_count
        data[8..12].copy_from_slice(&0u32.to_le_bytes()); // unknown
        data[12..16].copy_from_slice(&40u32.to_le_bytes()); // table_offset
        data[16..24].copy_from_slice(&65536u64.to_le_bytes()); // uncompressed_len
        data[24..32].copy_from_slice(&40000u64.to_le_bytes()); // compressed_len
        data[32..40].copy_from_slice(&32768u64.to_le_bytes()); // block_len
        // Block addresses
        data[40..48].copy_from_slice(&0u64.to_le_bytes());
        data[48..56].copy_from_slice(&20000u64.to_le_bytes());
        data[56..64].copy_from_slice(&40000u64.to_le_bytes());

        let rt = parse_lzx_reset_table(&data).unwrap();
        assert_eq!(rt.block_count, 2);
        assert_eq!(rt.block_len, 32768);
        assert_eq!(rt.uncompressed_len, 65536);
        assert_eq!(rt.block_addresses.len(), 3);
        assert_eq!(rt.block_addresses[0], 0);
        assert_eq!(rt.block_addresses[1], 20000);
        assert_eq!(rt.block_addresses[2], 40000);
    }

    #[test]
    fn lzxc_control_data_chm_v2() {
        // size=28, LZXC, version=2, reset_interval=4 (*0x8000=131072),
        // window_size=2 (*0x8000=65536), windows_per_reset=2
        let mut data = vec![0u8; 28];
        data[0..4].copy_from_slice(&28u32.to_le_bytes());
        data[4..8].copy_from_slice(b"LZXC");
        data[8..12].copy_from_slice(&2u32.to_le_bytes()); // version
        data[12..16].copy_from_slice(&4u32.to_le_bytes()); // reset_interval
        data[16..20].copy_from_slice(&2u32.to_le_bytes()); // window_size
        data[20..24].copy_from_slice(&2u32.to_le_bytes()); // windows_per_reset

        let ctrl = parse_lzxc_control_data_chm(&data).unwrap();
        assert_eq!(ctrl.window_size, 65536); // 2 * 0x8000
        assert_eq!(ctrl.reset_interval, 131072); // 4 * 0x8000
    }

    #[test]
    fn lzxc_control_data_chm_v1() {
        let mut data = vec![0u8; 28];
        data[0..4].copy_from_slice(&28u32.to_le_bytes());
        data[4..8].copy_from_slice(b"LZXC");
        data[8..12].copy_from_slice(&1u32.to_le_bytes()); // version 1
        data[12..16].copy_from_slice(&131072u32.to_le_bytes()); // already in bytes
        data[16..20].copy_from_slice(&65536u32.to_le_bytes());
        data[20..24].copy_from_slice(&2u32.to_le_bytes());

        let ctrl = parse_lzxc_control_data_chm(&data).unwrap();
        assert_eq!(ctrl.window_size, 65536);
        assert_eq!(ctrl.reset_interval, 131072);
    }

    #[test]
    fn lzxc_control_invalid_signature() {
        let mut data = vec![0u8; 28];
        data[4..8].copy_from_slice(b"NOPE");
        assert!(parse_lzxc_control_data_chm(&data).is_err());
    }
}
