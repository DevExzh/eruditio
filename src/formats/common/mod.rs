pub mod compression;
pub mod html_utils;
pub(crate) mod intrinsics;
pub mod itss;
pub mod palm_db;
pub mod text_utils;
pub mod xml_utils;
pub mod zip_utils;

/// Maximum decompressed input size for format readers (256 MB).
///
/// Applied via `Read::take()` before `read_to_end()` / `read_to_string()`
/// to prevent out-of-memory conditions from malicious or corrupted files.
pub const MAX_INPUT_SIZE: u64 = 256 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_input_size_is_256_mb() {
        assert_eq!(MAX_INPUT_SIZE, 256 * 1024 * 1024);
        assert_eq!(MAX_INPUT_SIZE, 268_435_456);
    }
}
