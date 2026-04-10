pub mod compression;
pub mod html_utils;
pub(crate) mod intrinsics;
pub mod itss;
pub mod palm_db;
pub mod text_utils;
pub mod xml_utils;
pub mod zip_utils;

use std::io::Read;

/// Maximum decompressed input size for format readers (256 MB).
///
/// Applied via `Read::take()` before `read_to_end()` / `read_to_string()`
/// to prevent out-of-memory conditions from malicious or corrupted files.
pub const MAX_INPUT_SIZE: u64 = 256 * 1024 * 1024;

/// Default initial buffer capacity for `read_capped`.
/// 256 KB reduces Vec-doubling overhead for typical ebook files (100 KB – 10 MB).
/// A 1 MB file now needs only 2 doublings (256K→512K→1M) instead of 4 with 64 KB.
/// The transient over-allocation for small files (<256 KB) is acceptable because
/// the buffer is consumed or freed immediately after the read completes.
const DEFAULT_READ_CAPACITY: usize = 256 * 1024;

/// Reads all bytes from `reader` up to `MAX_INPUT_SIZE`, pre-allocating
/// the buffer to reduce Vec doubling overhead.
pub fn read_capped(reader: &mut dyn Read) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::with_capacity(DEFAULT_READ_CAPACITY);
    (&mut *reader)
        .take(MAX_INPUT_SIZE)
        .read_to_end(&mut buffer)?;
    Ok(buffer)
}

/// Reads all bytes from `reader` as a UTF-8 string, up to `MAX_INPUT_SIZE`.
///
/// Uses `read_capped` for the initial read (pre-allocated 256 KB buffer), then
/// tries an ASCII fast-path (skips validation entirely when all bytes < 0x80)
/// before falling back to single-pass `String::from_utf8` validation.
/// Falls back to lossy conversion for non-UTF-8 content.
pub fn read_string_capped(reader: &mut dyn Read) -> std::io::Result<String> {
    let bytes = read_capped(reader)?;
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_input_size_is_256_mb() {
        assert_eq!(MAX_INPUT_SIZE, 256 * 1024 * 1024);
        assert_eq!(MAX_INPUT_SIZE, 268_435_456);
    }
}
