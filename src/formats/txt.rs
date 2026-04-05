use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::Result;
use crate::formats::common::html_utils::{strip_leading_heading, strip_tags, unescape_basic_entities};
use std::io::{Read, Write};

/// TXT format reader.
#[derive(Default)]
pub struct TxtReader;

impl TxtReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for TxtReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut contents = String::new();
        reader.read_to_string(&mut contents)?;

        let mut book = Book::new();

        // Simple plain text to HTML conversion
        let mut html_content = String::new();
        let mut blank_count = 0;

        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                blank_count += 1;
                if blank_count == 2 {
                    html_content.push_str("<p>&nbsp;</p>\n");
                }
            } else {
                blank_count = 0;
                html_content.push_str("<p>");

                // Basic XML escaping
                let escaped = trimmed
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");

                html_content.push_str(&escaped);
                html_content.push_str("</p>\n");
            }
        }

        book.add_chapter(&Chapter {
            title: Some("Main Content".into()),
            content: html_content,
            id: Some("main".into()),
        });

        book.metadata.title = Some("Unknown TXT Document".into());

        Ok(book)
    }
}

/// TXT format writer.
#[derive(Default)]
pub struct TxtWriter;

impl TxtWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for TxtWriter {
    fn write_book(&self, book: &Book, writer: &mut dyn Write) -> Result<()> {
        let text = book_to_plain_text(book);
        writer.write_all(text.as_bytes())?;
        Ok(())
    }
}

/// Converts a `Book` to plain text by stripping HTML from all chapters.
pub fn book_to_plain_text(book: &Book) -> String {
    let chapters = book.chapters();
    let mut parts = Vec::with_capacity(chapters.len());

    for chapter in &chapters {
        if let Some(ref title) = chapter.title {
            parts.push(title.clone());
            parts.push(String::new()); // blank line after title
        }
        let content = match chapter.title {
            Some(ref title) => strip_leading_heading(&chapter.content, title),
            None => &chapter.content,
        };
        let plain = strip_tags(content);
        let decoded = unescape_basic_entities(&plain);
        let trimmed = decoded.trim();
        if !trimmed.is_empty() {
            parts.push(trimmed.to_string());
        }
        parts.push(String::new()); // blank line between chapters
    }

    // Remove trailing empty lines.
    while parts.last().is_some_and(|s| s.is_empty()) {
        parts.pop();
    }

    let mut result = parts.join("\n");
    result.push('\n');
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn txt_writer_produces_plain_text() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello <b>world</b></p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(&Chapter {
            title: Some("Chapter 2".into()),
            content: "<p>Goodbye</p>".into(),
            id: Some("ch2".into()),
        });

        let mut output = Vec::new();
        TxtWriter::new().write_book(&book, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();

        assert!(text.contains("Chapter 1"));
        assert!(text.contains("Hello world"));
        assert!(text.contains("Chapter 2"));
        assert!(text.contains("Goodbye"));
    }

    #[test]
    fn txt_round_trip() {
        let input = "Hello World\n\nSecond paragraph\n";
        let mut cursor = Cursor::new(input.as_bytes());
        let book = TxtReader::new().read_book(&mut cursor).unwrap();

        let mut output = Vec::new();
        TxtWriter::new().write_book(&book, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();

        assert!(text.contains("Hello World"));
        assert!(text.contains("Second paragraph"));
    }

    #[test]
    fn txt_writer_no_duplicate_heading() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: Some("Ch 1".into()),
            content: "<h1>Ch 1</h1><p>Body text</p>".into(),
            id: Some("ch1".into()),
        });

        let text = book_to_plain_text(&book);
        // The title "Ch 1" should appear exactly once.
        let count = text.matches("Ch 1").count();
        assert_eq!(count, 1, "Expected 'Ch 1' once, but found {count} times in: {text}");
        assert!(text.contains("Body text"));
    }

    #[test]
    fn txt_writer_decodes_html_entities() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: Some("Test".into()),
            content: "<p>&amp; &lt; &gt; &quot; &#8212; &#8220;curly&#8221; &#169; &#174;</p>".into(),
            id: Some("ch1".into()),
        });
        let text = book_to_plain_text(&book);
        assert!(
            text.contains("& < > \" \u{2014} \u{201C}curly\u{201D} \u{00A9} \u{00AE}"),
            "Expected decoded entities in: {text}"
        );
    }
}
