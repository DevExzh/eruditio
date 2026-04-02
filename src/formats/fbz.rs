use crate::domain::{Book, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use crate::formats::fb2::{Fb2Reader, Fb2Writer};
use std::io::{Cursor, Read, Seek, Write};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// FBZ format reader (FB2 inside a ZIP archive).
#[derive(Default)]
pub struct FbzReader;

impl FbzReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for FbzReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).map_err(EruditioError::Io)?;
        let cursor = Cursor::new(buffer);

        let mut archive = ZipArchive::new(cursor)
            .map_err(|e| EruditioError::Format(format!("Failed to open FBZ as ZIP: {}", e)))?;

        // Find the first .fb2 file in the archive.
        let fb2_name = find_fb2_file(&mut archive)
            .ok_or_else(|| EruditioError::Format("No .fb2 file found in FBZ archive".into()))?;

        let mut fb2_file = archive
            .by_name(&fb2_name)
            .map_err(|_| EruditioError::Format(format!("Failed to read {}", fb2_name)))?;

        let mut contents = Vec::new();
        fb2_file
            .read_to_end(&mut contents)
            .map_err(EruditioError::Io)?;

        let mut cursor = Cursor::new(contents);
        Fb2Reader::new().read_book(&mut cursor)
    }
}

/// FBZ format writer (FB2 inside a ZIP archive).
#[derive(Default)]
pub struct FbzWriter;

impl FbzWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for FbzWriter {
    fn write_book(&self, book: &Book, output: &mut dyn Write) -> Result<()> {
        // Write FB2 content to memory first.
        let mut fb2_buf = Vec::new();
        Fb2Writer::new().write_book(book, &mut fb2_buf)?;

        // Wrap in a ZIP.
        let mut zip_buf = Cursor::new(Vec::new());
        write_single_file_zip(&mut zip_buf, "content.fb2", &fb2_buf)?;

        output
            .write_all(zip_buf.get_ref())
            .map_err(EruditioError::Io)
    }
}

/// Finds the first .fb2 file in a ZIP archive.
fn find_fb2_file<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Option<String> {
    for i in 0..archive.len() {
        if let Ok(file) = archive.by_index(i) {
            let name = file.name().to_string();
            if name.to_lowercase().ends_with(".fb2") {
                return Some(name);
            }
        }
    }
    None
}

/// Creates a ZIP archive containing a single file.
fn write_single_file_zip<W: Write + Seek>(
    writer: &mut W,
    filename: &str,
    data: &[u8],
) -> Result<()> {
    let mut zip = ZipWriter::new(writer);
    let options: FileOptions<'_, ()> =
        FileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file(filename, options)
        .map_err(|e| EruditioError::Format(format!("Failed to create {}: {}", filename, e)))?;
    zip.write_all(data).map_err(EruditioError::Io)?;
    zip.finish()
        .map_err(|e| EruditioError::Format(format!("Failed to finalize ZIP: {}", e)))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;

    #[test]
    fn fbz_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("FBZ Test".into());
        book.metadata.authors.push("Test Author".into());
        book.metadata.language = Some("en".into());
        book.add_chapter(&Chapter {
            title: Some("Section 1".into()),
            content: "<p>Hello from FBZ</p>".into(),
            id: Some("s1".into()),
        });

        // Write
        let mut output = Vec::new();
        FbzWriter::new().write_book(&book, &mut output).unwrap();

        // Read back
        let mut cursor = Cursor::new(output);
        let decoded = FbzReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("FBZ Test"));
        let chapters = decoded.chapters();
        assert!(!chapters.is_empty());
    }
}
