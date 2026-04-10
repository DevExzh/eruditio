//! PMLZ format reader and writer — PML inside a ZIP archive.
//!
//! A PMLZ file is a ZIP containing a `.pml` file and optionally images.

use crate::domain::{Book, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use crate::formats::common::MAX_INPUT_SIZE;
use crate::formats::common::text_utils::ends_with_ascii_ci;
use crate::formats::common::zip_utils::ZIP_DEFLATE_LEVEL;
use crate::formats::pml::{PmlReader, PmlWriter};
use std::io::{Cursor, Read, Write};

/// PMLZ format reader.
#[derive(Default)]
pub struct PmlzReader;

impl PmlzReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for PmlzReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut data = Vec::new();
        (&mut *reader).take(MAX_INPUT_SIZE).read_to_end(&mut data)?;

        let cursor = Cursor::new(&data);
        let mut archive = zip::ZipArchive::new(cursor)?;

        // Find the .pml file inside the ZIP.
        let pml_name = (0..archive.len())
            .filter_map(|i| {
                let file = archive.by_index(i).ok()?;
                let name = file.name().to_string();
                if ends_with_ascii_ci(&name, ".pml") {
                    Some(name)
                } else {
                    None
                }
            })
            .next()
            .ok_or_else(|| EruditioError::Format("No .pml file found in PMLZ archive".into()))?;

        let mut pml_data = Vec::new();
        archive.by_name(&pml_name)?.read_to_end(&mut pml_data)?;

        let mut cursor = Cursor::new(pml_data);
        PmlReader::new().read_book(&mut cursor)
    }
}

/// PMLZ format writer.
#[derive(Default)]
pub struct PmlzWriter;

impl PmlzWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for PmlzWriter {
    fn write_book(&self, book: &Book, output: &mut dyn Write) -> Result<()> {
        // Write PML content to a buffer.
        let mut pml_buf = Vec::new();
        PmlWriter::new().write_book(book, &mut pml_buf)?;

        // Pack into a ZIP.
        // Skip deflate for small payloads to avoid ~256 KB zlib_rs state init.
        const MIN_DEFLATE_SIZE: usize = 4096;
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut cursor);
            let options = if pml_buf.len() >= MIN_DEFLATE_SIZE {
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .compression_level(ZIP_DEFLATE_LEVEL)
            } else {
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored)
            };
            zip.start_file("content.pml", options)?;
            zip.write_all(&pml_buf)?;
            zip.finish()?;
        }

        output.write_all(&cursor.into_inner())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;
    use std::io::Cursor;

    #[test]
    fn pmlz_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("PMLZ Test".into());
        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello PMLZ world</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        PmlzWriter::new().write_book(&book, &mut output).unwrap();

        let mut cursor = Cursor::new(output);
        let decoded = PmlzReader::new().read_book(&mut cursor).unwrap();
        let content: String = decoded
            .chapters()
            .iter()
            .map(|c| c.content.clone())
            .collect();
        assert!(content.contains("Hello PMLZ world"));
    }

    #[test]
    fn pmlz_rejects_empty_zip() {
        // Create a ZIP with no .pml file.
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("readme.txt", opts).unwrap();
            zip.write_all(b"no pml here").unwrap();
            zip.finish().unwrap();
        }

        let mut cursor = Cursor::new(buf.into_inner());
        let result = PmlzReader::new().read_book(&mut cursor);
        assert!(result.is_err());
    }
}
