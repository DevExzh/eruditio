//! Markdown ebook reader.
//!
//! Converts Markdown (`.md`, `.markdown`) to HTML using pulldown-cmark,
//! then wraps the result as a single-chapter Book.

use crate::domain::{Book, Chapter, FormatReader};
use crate::error::{EruditioError, Result};
use std::io::Read;

/// Markdown format reader.
#[derive(Default)]
pub struct MdReader;

impl MdReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for MdReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut input = String::new();
        reader
            .read_to_string(&mut input)
            .map_err(EruditioError::Io)?;

        let opts = pulldown_cmark::Options::ENABLE_TABLES
            | pulldown_cmark::Options::ENABLE_FOOTNOTES
            | pulldown_cmark::Options::ENABLE_STRIKETHROUGH
            | pulldown_cmark::Options::ENABLE_TASKLISTS
            | pulldown_cmark::Options::ENABLE_HEADING_ATTRIBUTES;

        let parser = pulldown_cmark::Parser::new_ext(&input, opts);
        let mut html = String::new();
        pulldown_cmark::html::push_html(&mut html, parser);

        let mut book = Book::new();

        // Try to extract a title from the first H1 heading.
        if let Some(start) = html.find("<h1") {
            if let Some(gt) = html[start..].find('>') {
                let after = start + gt + 1;
                if let Some(end) = html[after..].find("</h1>") {
                    let title = &html[after..after + end];
                    book.metadata.title = Some(title.to_string());
                }
            }
        }

        book.add_chapter(&Chapter {
            title: book.metadata.title.clone(),
            content: html,
            id: Some("md_content".into()),
        });

        Ok(book)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn basic_markdown_to_html() {
        let md = "# Hello\n\nThis is **bold** text.\n";
        let mut cursor = Cursor::new(md.as_bytes());
        let book = MdReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Hello"));
        let chapters = book.chapters();
        assert_eq!(chapters.len(), 1);
        assert!(chapters[0].content.contains("<strong>bold</strong>"));
    }

    #[test]
    fn markdown_with_table() {
        let md = "| A | B |\n|---|---|\n| 1 | 2 |\n";
        let mut cursor = Cursor::new(md.as_bytes());
        let book = MdReader::new().read_book(&mut cursor).unwrap();
        let chapters = book.chapters();
        assert!(chapters[0].content.contains("<table>"));
    }
}
