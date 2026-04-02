//! HTML format reader and writer.
//!
//! Reads HTML files into the `Book` intermediate representation and writes
//! books back as standalone HTML documents.

pub mod parser;

use base64::Engine;

use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use crate::formats::common::html_utils::escape_html;
use std::io::{Read, Write};

/// HTML format reader.
///
/// Parses an HTML document, extracting metadata from `<head>` and content
/// from `<body>`. Splits content into chapters at `<h1>`/`<h2>` boundaries.
#[derive(Default)]
pub struct HtmlReader;

impl HtmlReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for HtmlReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut contents = String::new();
        reader
            .read_to_string(&mut contents)
            .map_err(EruditioError::Io)?;

        let mut book = Book::new();

        // Extract metadata from <head>.
        book.metadata = parser::extract_metadata(&contents);

        // Extract body content.
        let body = parser::extract_body(&contents);

        // Split into chapters.
        let chapters = parser::split_into_chapters(&body);

        if chapters.is_empty() {
            // Fallback: treat entire body as one chapter.
            book.add_chapter(&Chapter {
                title: Some("Main Content".into()),
                content: body,
                id: Some("main".into()),
            });
        } else {
            for (i, (title, content)) in chapters.into_iter().enumerate() {
                book.add_chapter(&Chapter {
                    title,
                    content,
                    id: Some(format!("chapter_{}", i)),
                });
            }
        }

        // Default title if none found.
        if book.metadata.title.is_none() {
            book.metadata.title = Some("Unknown HTML Document".into());
        }

        Ok(book)
    }
}

/// HTML format writer.
///
/// Generates a standalone HTML5 document from a `Book`.
/// Chapters are written as sections with heading separators.
/// Images are embedded as base64 data URIs.
#[derive(Default)]
pub struct HtmlWriter;

impl HtmlWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for HtmlWriter {
    fn write_book(&self, book: &Book, writer: &mut dyn Write) -> Result<()> {
        let html = book_to_html(book);
        writer.write_all(html.as_bytes()).map_err(EruditioError::Io)
    }
}

/// Converts a `Book` to a standalone HTML5 document string.
fn book_to_html(book: &Book) -> String {
    let title = book.metadata.title.as_deref().unwrap_or("Untitled");
    let chapters = book.chapters();

    // Build body content.
    let mut body = String::with_capacity(4096);

    for (i, chapter) in chapters.iter().enumerate() {
        if i > 0 {
            body.push_str("<hr />\n");
        }

        if let Some(ref ch_title) = chapter.title {
            body.push_str(&format!("<h1>{}</h1>\n", escape_html(ch_title)));
        }

        body.push_str(&chapter.content);
        body.push('\n');
    }

    // Embed images as base64 data URIs in a style block.
    let resources = book.resources();
    if !resources.is_empty() {
        body.push_str("\n<!-- Embedded resources -->\n");
        for res in &resources {
            if res.media_type.starts_with("image/") {
                let b64 = base64::engine::general_purpose::STANDARD.encode(res.data);
                body.push_str(&format!(
                    "<img src=\"data:{};base64,{}\" alt=\"{}\" />\n",
                    res.media_type,
                    b64,
                    escape_html(res.id),
                ));
            }
        }
    }

    parser::build_html_document(title, &book.metadata, &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn html_reader_extracts_metadata() {
        let html = r#"<!DOCTYPE html>
<html>
<head>
<title>Test Book</title>
<meta name="author" content="Alice">
<meta name="language" content="en">
</head>
<body>
<p>Hello world</p>
</body>
</html>"#;

        let mut cursor = Cursor::new(html.as_bytes());
        let book = HtmlReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Test Book"));
        assert_eq!(book.metadata.authors, vec!["Alice"]);
        assert_eq!(book.metadata.language.as_deref(), Some("en"));
    }

    #[test]
    fn html_reader_splits_chapters() {
        let html = r#"<html><head><title>T</title></head><body>
<h1>Chapter 1</h1><p>Content one</p>
<h1>Chapter 2</h1><p>Content two</p>
</body></html>"#;

        let mut cursor = Cursor::new(html.as_bytes());
        let book = HtmlReader::new().read_book(&mut cursor).unwrap();

        let chapters = book.chapters();
        assert_eq!(chapters.len(), 2);
        assert!(chapters[0].content.contains("Content one"));
        assert!(chapters[1].content.contains("Content two"));
    }

    #[test]
    fn html_reader_handles_fragment() {
        let html = "<p>Just a paragraph</p>";
        let mut cursor = Cursor::new(html.as_bytes());
        let book = HtmlReader::new().read_book(&mut cursor).unwrap();

        assert!(!book.chapters().is_empty());
        assert!(book.chapters()[0].content.contains("Just a paragraph"));
    }

    #[test]
    fn html_writer_produces_valid_html() {
        let mut book = Book::new();
        book.metadata.title = Some("My Book".into());
        book.metadata.authors.push("Bob".into());
        book.add_chapter(&Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello world</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        HtmlWriter::new().write_book(&book, &mut output).unwrap();
        let html = String::from_utf8(output).unwrap();

        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<title>My Book</title>"));
        assert!(html.contains("name=\"author\""));
        assert!(html.contains("content=\"Bob\""));
        assert!(html.contains("<h1>Chapter 1</h1>"));
        assert!(html.contains("Hello world"));
    }

    #[test]
    fn html_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("Round Trip".into());
        book.metadata.authors.push("Author".into());
        book.add_chapter(&Chapter {
            title: Some("Ch 1".into()),
            content: "<p>Content here</p>".into(),
            id: Some("ch1".into()),
        });

        // Write
        let mut output = Vec::new();
        HtmlWriter::new().write_book(&book, &mut output).unwrap();

        // Read back
        let mut cursor = Cursor::new(output);
        let decoded = HtmlReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("Round Trip"));
        assert!(!decoded.chapters().is_empty());
    }

    #[test]
    fn html_writer_embeds_images() {
        let mut book = Book::new();
        book.metadata.title = Some("Image Test".into());
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>text</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_resource("img1", "cover.png", vec![0x89, 0x50], "image/png");

        let mut output = Vec::new();
        HtmlWriter::new().write_book(&book, &mut output).unwrap();
        let html = String::from_utf8(output).unwrap();

        assert!(html.contains("data:image/png;base64,"));
    }
}
