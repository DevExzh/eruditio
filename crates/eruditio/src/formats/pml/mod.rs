//! PML (Palm Markup Language) format reader and writer.
//!
//! PML is a simple tag-based markup used by Palm eReader devices.
//! Tags use backslash notation (`\b` for bold, `\i` for italic, etc.).
//! PMLZ wraps a PML file inside a ZIP archive.

pub mod parser;

use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::Result;
use std::io::{Read, Write};

/// PML format reader.
#[derive(Default)]
pub struct PmlReader;

impl PmlReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for PmlReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;

        let text = crate::formats::common::text_utils::bytes_to_cow_str(&data);
        let html = parser::pml_to_html(&text);
        let chapters = parser::split_pml_chapters(&html);

        let mut book = Book::new();
        book.metadata.title = Some("Unknown PML Document".into());

        if chapters.is_empty() {
            book.add_chapter(Chapter {
                title: None,
                content: html,
                id: Some("main".into()),
            });
        } else {
            for (i, (title, content)) in chapters.into_iter().enumerate() {
                book.add_chapter(Chapter {
                    title,
                    content,
                    id: Some(format!("chapter_{}", i)),
                });
            }
        }

        Ok(book)
    }
}

/// PML format writer.
#[derive(Default)]
pub struct PmlWriter;

impl PmlWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for PmlWriter {
    fn write_book(&self, book: &Book, output: &mut dyn Write) -> Result<()> {
        let pml = parser::book_to_pml(book);
        output.write_all(pml.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn pml_reader_parses_basic_text() {
        let pml = b"\\x Chapter One\nHello world\n\\p\n\\x Chapter Two\nGoodbye";
        let mut cursor = Cursor::new(pml.as_slice());
        let book = PmlReader::new().read_book(&mut cursor).unwrap();

        let chapters = book.chapters();
        assert!(chapters.len() >= 2);
    }

    #[test]
    fn pml_reader_handles_formatting() {
        let pml = b"\\bBold text\\b and \\iitalic text\\i here";
        let mut cursor = Cursor::new(pml.as_slice());
        let book = PmlReader::new().read_book(&mut cursor).unwrap();

        let content: String = book.chapter_views().iter().map(|c| c.content).collect();
        assert!(content.contains("<b>") || content.contains("font-weight"));
        assert!(content.contains("Bold text"));
    }

    #[test]
    fn pml_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("PML Test".into());
        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello world</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        PmlWriter::new().write_book(&book, &mut output).unwrap();

        let mut cursor = Cursor::new(output);
        let decoded = PmlReader::new().read_book(&mut cursor).unwrap();
        let content: String = decoded
            .chapters()
            .iter()
            .map(|c| c.content.clone())
            .collect();
        assert!(content.contains("Hello world"));
    }

    #[test]
    fn pml_writer_produces_pml_markup() {
        let mut book = Book::new();
        book.metadata.title = Some("Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch 1".into()),
            content: "<p><b>Bold</b> and <i>italic</i></p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        PmlWriter::new().write_book(&book, &mut output).unwrap();
        let pml = String::from_utf8(output).unwrap();

        assert!(pml.contains("\\b"));
        assert!(pml.contains("\\i"));
        assert!(pml.contains("Bold"));
        assert!(pml.contains("italic"));
    }
}
