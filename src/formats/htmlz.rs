//! HTMLZ format: HTML inside a ZIP archive.
//!
//! Delegates to `HtmlReader`/`HtmlWriter` for the actual content,
//! wrapping/unwrapping the ZIP container.

use crate::domain::{Book, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use crate::formats::html::{HtmlReader, HtmlWriter};
use std::io::{Cursor, Read, Seek, Write};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// HTMLZ format reader (HTML inside a ZIP archive).
#[derive(Default)]
pub struct HtmlzReader;

impl HtmlzReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for HtmlzReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer)?;
        let cursor = Cursor::new(buffer);

        let mut archive = ZipArchive::new(cursor)
            .map_err(|e| EruditioError::Format(format!("Failed to open HTMLZ as ZIP: {}", e)))?;

        // Find the first HTML file in the archive.
        let html_name = find_html_file(&mut archive)
            .ok_or_else(|| EruditioError::Format("No HTML file found in HTMLZ archive".into()))?;

        let mut html_file = archive
            .by_name(&html_name)
            .map_err(|_| EruditioError::Format(format!("Failed to read {}", html_name)))?;

        let mut contents = Vec::new();
        html_file.read_to_end(&mut contents)?;

        let mut cursor = Cursor::new(contents);
        HtmlReader::new().read_book(&mut cursor)
    }
}

/// HTMLZ format writer (HTML inside a ZIP archive).
#[derive(Default)]
pub struct HtmlzWriter;

impl HtmlzWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for HtmlzWriter {
    fn write_book(&self, book: &Book, output: &mut dyn Write) -> Result<()> {
        // Write HTML content to memory first.
        let mut html_buf = Vec::new();
        HtmlWriter::new().write_book(book, &mut html_buf)?;

        // Wrap in a ZIP.
        let mut zip_buf = Cursor::new(Vec::new());
        write_single_file_zip(&mut zip_buf, "index.html", &html_buf)?;

        output.write_all(zip_buf.get_ref())?;
        Ok(())
    }
}

/// Finds the first HTML file in a ZIP archive.
fn find_html_file<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Option<String> {
    for i in 0..archive.len() {
        if let Ok(file) = archive.by_index(i) {
            let name = file.name().to_lowercase();
            if name.ends_with(".html") || name.ends_with(".htm") || name.ends_with(".xhtml") {
                return Some(file.name().to_string());
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
    fn htmlz_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("HTMLZ Test".into());
        book.metadata.authors.push("Test Author".into());
        book.add_chapter(&Chapter {
            title: Some("Section 1".into()),
            content: "<p>Hello from HTMLZ</p>".into(),
            id: Some("s1".into()),
        });

        // Write
        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        // Read back
        let mut cursor = Cursor::new(output);
        let decoded = HtmlzReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("HTMLZ Test"));
        let chapters = decoded.chapters();
        assert!(!chapters.is_empty());
    }

    #[test]
    fn htmlz_preserves_metadata() {
        let mut book = Book::new();
        book.metadata.title = Some("Meta Test".into());
        book.metadata.authors.push("Alice".into());
        book.metadata.language = Some("en".into());
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        let mut cursor = Cursor::new(output);
        let decoded = HtmlzReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("Meta Test"));
        assert_eq!(decoded.metadata.authors, vec!["Alice"]);
        assert_eq!(decoded.metadata.language.as_deref(), Some("en"));
    }
}
