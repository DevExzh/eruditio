use crate::domain::{Book, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use crate::formats::txt::{TxtReader, TxtWriter};
use std::io::{Cursor, Read, Seek, Write};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// TXTZ format reader (TXT inside a ZIP archive).
#[derive(Default)]
pub struct TxtzReader;

impl TxtzReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for TxtzReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer)?;
        let cursor = Cursor::new(buffer);

        let mut archive = ZipArchive::new(cursor)
            .map_err(|e| EruditioError::Format(format!("Failed to open TXTZ as ZIP: {}", e)))?;

        // Find the first .txt file in the archive.
        let txt_name = find_file_by_extension(&mut archive, "txt")
            .ok_or_else(|| EruditioError::Format("No .txt file found in TXTZ archive".into()))?;

        let mut txt_file = archive
            .by_name(&txt_name)
            .map_err(|_| EruditioError::Format(format!("Failed to read {}", txt_name)))?;

        let mut contents = Vec::new();
        txt_file.read_to_end(&mut contents)?;

        let mut cursor = Cursor::new(contents);
        TxtReader::new().read_book(&mut cursor)
    }
}

/// TXTZ format writer (TXT inside a ZIP archive).
#[derive(Default)]
pub struct TxtzWriter;

impl TxtzWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for TxtzWriter {
    fn write_book(&self, book: &Book, output: &mut dyn Write) -> Result<()> {
        // Write TXT content to memory first.
        let mut txt_buf = Vec::new();
        TxtWriter::new().write_book(book, &mut txt_buf)?;

        // Wrap in a ZIP.
        let mut zip_buf = Cursor::new(Vec::new());
        write_single_file_zip(&mut zip_buf, "content.txt", &txt_buf)?;

        output.write_all(zip_buf.get_ref())?;
        Ok(())
    }
}

/// Finds the first file with a given extension in a ZIP archive.
fn find_file_by_extension<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    ext: &str,
) -> Option<String> {
    for i in 0..archive.len() {
        if let Ok(file) = archive.by_index(i) {
            let name = file.name().to_string();
            if name.to_lowercase().ends_with(&format!(".{}", ext)) {
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
    zip.write_all(data)?;
    zip.finish()
        .map_err(|e| EruditioError::Format(format!("Failed to finalize ZIP: {}", e)))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;

    #[test]
    fn txtz_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("TXTZ Test".into());
        book.add_chapter(&Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello from TXTZ</p>".into(),
            id: Some("ch1".into()),
        });

        // Write
        let mut output = Vec::new();
        TxtzWriter::new().write_book(&book, &mut output).unwrap();

        // Read back
        let mut cursor = Cursor::new(output);
        let decoded = TxtzReader::new().read_book(&mut cursor).unwrap();
        let chapters = decoded.chapters();
        assert!(!chapters.is_empty());

        // Content should be preserved (as plain text)
        let mut all_text = String::new();
        for ch in &chapters {
            all_text.push_str(&ch.content);
        }
        assert!(all_text.contains("Hello from TXTZ"));
    }
}
