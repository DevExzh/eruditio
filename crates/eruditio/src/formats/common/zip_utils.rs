use crate::error::{EruditioError, Result};
use std::io::{Read, Seek};
use zip::ZipArchive;

/// Maximum decompressed size for a single ZIP entry (256 MB).
const MAX_ZIP_ENTRY: u64 = 256 * 1024 * 1024;

/// Default ZIP deflate compression level for all ebook writers.
///
/// Level 1 (fastest) is ~2-3× faster than level 6 (default) with only 5-10%
/// larger output on text-heavy ebook content (XHTML, CSS, NCX).  Since ebook
/// archives are typically < 5 MB and images use `Stored`, the absolute size
/// difference is negligible (a few KB).
pub(crate) const ZIP_DEFLATE_LEVEL: Option<i64> = Some(1);

/// Reads a file from a ZIP archive by name, returning its contents as a `String`.
pub fn read_zip_text<R: Read + Seek>(archive: &mut ZipArchive<R>, name: &str) -> Result<String> {
    let file = archive
        .by_name(name)
        .map_err(|_| EruditioError::Format(format!("File not found in archive: {}", name)))?;

    let size_hint = (file.size() as usize).min(MAX_ZIP_ENTRY as usize);
    let mut contents = String::with_capacity(size_hint);
    file.take(MAX_ZIP_ENTRY).read_to_string(&mut contents)?;

    Ok(contents)
}

/// Reads a file from a ZIP archive by name, returning its contents as raw bytes.
pub fn read_zip_bytes<R: Read + Seek>(archive: &mut ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let file = archive
        .by_name(name)
        .map_err(|_| EruditioError::Format(format!("File not found in archive: {}", name)))?;

    let size_hint = (file.size() as usize).min(MAX_ZIP_ENTRY as usize);
    let mut data = Vec::with_capacity(size_hint);
    file.take(MAX_ZIP_ENTRY).read_to_end(&mut data)?;

    Ok(data)
}

/// Lists all file names in a ZIP archive.
pub fn list_zip_entries<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Vec<String> {
    (0..archive.len())
        .filter_map(|i| {
            archive.by_index(i).ok().and_then(|f| {
                if f.is_file() {
                    Some(f.name().to_string())
                } else {
                    None
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use zip::ZipWriter;
    use zip::write::FileOptions;

    fn create_test_zip() -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut zip = ZipWriter::new(cursor);
            let options: FileOptions<'_, ()> =
                FileOptions::default().compression_method(zip::CompressionMethod::Stored);

            zip.start_file("hello.txt", options).unwrap();
            std::io::Write::write_all(&mut zip, b"Hello World").unwrap();

            zip.start_file("data.bin", options).unwrap();
            std::io::Write::write_all(&mut zip, &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();

            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn read_text_file_from_zip() {
        let data = create_test_zip();
        let cursor = Cursor::new(data);
        let mut archive = ZipArchive::new(cursor).unwrap();

        let text = read_zip_text(&mut archive, "hello.txt").unwrap();
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn read_binary_file_from_zip() {
        let data = create_test_zip();
        let cursor = Cursor::new(data);
        let mut archive = ZipArchive::new(cursor).unwrap();

        let bytes = read_zip_bytes(&mut archive, "data.bin").unwrap();
        assert_eq!(bytes, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn missing_file_returns_error() {
        let data = create_test_zip();
        let cursor = Cursor::new(data);
        let mut archive = ZipArchive::new(cursor).unwrap();

        let result = read_zip_text(&mut archive, "missing.txt");
        assert!(result.is_err());
    }

    #[test]
    fn list_entries() {
        let data = create_test_zip();
        let cursor = Cursor::new(data);
        let mut archive = ZipArchive::new(cursor).unwrap();

        let entries = list_zip_entries(&mut archive);
        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&"hello.txt".to_string()));
        assert!(entries.contains(&"data.bin".to_string()));
    }
}
