use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::Result;
use crate::formats::common::MAX_INPUT_SIZE;
use crate::formats::common::html_utils::{strip_tags, unescape_basic_entities};
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
        (&mut *reader)
            .take(MAX_INPUT_SIZE)
            .read_to_string(&mut contents)?;

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

/// Strips everything up to and including the first occurrence of `title` in the
/// plain text, but only if the title appears within the first ~500 characters.
///
/// This handles Gutenberg-style EPUBs where a page-header div appears before
/// the chapter heading, which causes `strip_leading_heading` (HTML-level) to
/// miss the heading.  By operating on the already-flattened plain text we can
/// remove both the boilerplate *and* the duplicate heading in one pass.
///
/// Both the title and the search area are whitespace-normalised and lowercased
/// for comparison, so `<br/>` newlines in headings still match.
fn strip_title_prefix<'a>(text: &'a str, title: &str) -> &'a str {
    let normalised_title = title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if normalised_title.is_empty() {
        return text;
    }
    // Only search within the first ~500 bytes to avoid stripping content
    // mid-chapter on a false match.  Adjust to a valid char boundary to
    // avoid panicking on multi-byte UTF-8 sequences.
    let mut search_limit = text.len().min(500);
    while search_limit > 0 && !text.is_char_boundary(search_limit) {
        search_limit -= 1;
    }
    let search_area = &text[..search_limit];

    // Build a whitespace-normalised, lowercased version of the search area
    // together with a mapping from each normalised byte position back to the
    // original byte index that follows the corresponding character.
    let mut normalised = String::new();
    // `orig_end[i]` = the byte offset in `text` right after the original
    // character that produced normalised byte `i`.
    let mut orig_end: Vec<usize> = Vec::new();
    let mut in_ws = false;
    for (byte_pos, ch) in search_area.char_indices() {
        if ch.is_whitespace() {
            if !in_ws && !normalised.is_empty() {
                normalised.push(' ');
                // The space maps to right after this whitespace char.
                let end = byte_pos + ch.len_utf8();
                for _ in 0..' '.len_utf8() {
                    orig_end.push(end);
                }
                in_ws = true;
            }
            // Skip additional whitespace (update the mapped position of
            // the trailing normalised space so it points past the last ws).
            if in_ws && !normalised.is_empty() {
                let end = byte_pos + ch.len_utf8();
                let n = orig_end.len();
                if n > 0 {
                    orig_end[n - 1] = end;
                }
            }
        } else {
            in_ws = false;
            for lower in ch.to_lowercase() {
                let start_n = normalised.len();
                normalised.push(lower);
                let bytes_added = normalised.len() - start_n;
                let end = byte_pos + ch.len_utf8();
                for _ in 0..bytes_added {
                    orig_end.push(end);
                }
            }
        }
    }

    if let Some(pos) = normalised.find(&normalised_title) {
        let end_normalised = pos + normalised_title.len();
        // Map back to the original text position.
        let orig_after = if end_normalised > 0 && end_normalised <= orig_end.len() {
            orig_end[end_normalised - 1]
        } else {
            0
        };
        text[orig_after..].trim_start()
    } else {
        text
    }
}

/// Returns `true` if the chapter is essentially just a cover image with no
/// substantial text content.  Cover-only chapters typically contain a single
/// `<img>` tag whose alt text is "Cover" (or similar short text) and nothing
/// else of interest.  We detect this by checking:
///   - the chapter title is "Cover" or empty, AND
///   - the stripped plain-text content is very short (under ~20 chars).
fn is_cover_only_chapter(chapter: &Chapter) -> bool {
    let title_is_cover = match chapter.title {
        Some(ref t) => {
            let t = t.trim();
            t.is_empty() || t.eq_ignore_ascii_case("cover")
        },
        None => true,
    };
    if !title_is_cover {
        return false;
    }
    let plain = strip_tags(&chapter.content);
    let decoded = unescape_basic_entities(&plain);
    let trimmed = decoded.trim();
    trimmed.len() < 20
}

/// Converts a `Book` to plain text by stripping HTML from all chapters.
pub fn book_to_plain_text(book: &Book) -> String {
    let chapters = book.chapters();
    let mut parts = Vec::with_capacity(chapters.len());

    for chapter in &chapters {
        // Skip cover-only chapters to avoid "Cover" alt-text artifacts.
        if is_cover_only_chapter(chapter) {
            continue;
        }

        if let Some(ref title) = chapter.title {
            parts.push(title.clone());
            parts.push(String::new()); // blank line after title
        }
        let plain = strip_tags(&chapter.content);
        let decoded = unescape_basic_entities(&plain);
        let trimmed = decoded.trim();
        // For TXT output, strip everything up to and including the title
        // to remove both page-header boilerplate and duplicate heading.
        let trimmed = match chapter.title {
            Some(ref title) => strip_title_prefix(trimmed, title),
            None => trimmed,
        };
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
        assert_eq!(
            count, 1,
            "Expected 'Ch 1' once, but found {count} times in: {text}"
        );
        assert!(text.contains("Body text"));
    }

    #[test]
    fn txt_writer_decodes_html_entities() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: Some("Test".into()),
            content: "<p>&amp; &lt; &gt; &quot; &#8212; &#8220;curly&#8221; &#169; &#174;</p>"
                .into(),
            id: Some("ch1".into()),
        });
        let text = book_to_plain_text(&book);
        assert!(
            text.contains("& < > \" \u{2014} \u{201C}curly\u{201D} \u{00A9} \u{00AE}"),
            "Expected decoded entities in: {text}"
        );
    }

    #[test]
    fn txt_writer_no_duplicate_heading_with_br() {
        // Heading in HTML body contains <br/> which previously caused strip_leading_heading
        // to fail matching, resulting in the title appearing twice.
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: Some("CHAPTER I. Down the Rabbit-Hole".into()),
            content: "<h1>CHAPTER I.<br/>Down the Rabbit-Hole</h1><p>Alice was beginning to get very tired.</p>".into(),
            id: Some("ch1".into()),
        });
        let text = book_to_plain_text(&book);
        let count = text.matches("Down the Rabbit-Hole").count();
        assert_eq!(
            count, 1,
            "Expected 'Down the Rabbit-Hole' once, but found {count} times in:\n{text}"
        );
        assert!(text.contains("Alice was beginning to get very tired."));
    }

    #[test]
    fn txt_writer_no_triplicate_heading_with_pgheader() {
        // Gutenberg EPUBs include a page-header div before the chapter heading.
        // This used to produce 3 occurrences: explicit title, pgheader boilerplate
        // text, and the heading remaining in the body.
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: Some("CHAPTER I. Down the Rabbit-Hole".into()),
            content: r#"<div class="pg-boilerplate pgheader section">
                <h2>The Project Gutenberg eBook of Alice's Adventures in Wonderland</h2>
                <p>Release date: June 27, 2008</p>
            </div>
            <h2>CHAPTER I. Down the Rabbit-Hole</h2>
            <p>Alice was beginning to get very tired of sitting by her sister.</p>"#
                .into(),
            id: Some("ch1".into()),
        });

        let text = book_to_plain_text(&book);
        let count = text.matches("Down the Rabbit-Hole").count();
        assert_eq!(
            count, 1,
            "Expected 'Down the Rabbit-Hole' once, but found {count} times in:\n{text}"
        );
        // The pgheader boilerplate should also be stripped.
        assert!(
            !text.contains("Project Gutenberg"),
            "pgheader boilerplate should be stripped, but found in:\n{text}"
        );
        assert!(text.contains("Alice was beginning to get very tired"));
    }

    #[test]
    fn strip_title_prefix_basic() {
        let text = "Some boilerplate\n\nCHAPTER I. Down the Rabbit-Hole\n\nAlice was beginning";
        let result = strip_title_prefix(text, "CHAPTER I. Down the Rabbit-Hole");
        assert_eq!(result, "Alice was beginning");
    }

    #[test]
    fn strip_title_prefix_case_insensitive() {
        let text = "chapter i. down the rabbit-hole\n\nBody text";
        let result = strip_title_prefix(text, "CHAPTER I. Down the Rabbit-Hole");
        assert_eq!(result, "Body text");
    }

    #[test]
    fn strip_title_prefix_no_match() {
        let text = "This text has no chapter heading\n\nBody text";
        let result = strip_title_prefix(text, "CHAPTER I. Down the Rabbit-Hole");
        assert_eq!(result, text);
    }

    #[test]
    fn strip_title_prefix_empty_title() {
        let text = "Some text here";
        let result = strip_title_prefix(text, "");
        assert_eq!(result, text);
    }

    #[test]
    fn strip_title_prefix_whitespace_normalised() {
        // Both the title and the text are whitespace-normalised for matching,
        // so extra spaces/newlines in the text still match the title.
        let text = "CHAPTER   I.   Down  the  Rabbit-Hole\n\nBody text";
        let result = strip_title_prefix(text, "CHAPTER I. Down the Rabbit-Hole");
        assert_eq!(result, "Body text");
    }

    #[test]
    fn strip_title_prefix_multibyte_at_boundary() {
        // Regression: if a multi-byte UTF-8 character straddles byte 500,
        // the function must not panic.
        let mut text = "A".repeat(499);
        text.push('\u{00E9}'); // é = 2-byte UTF-8, straddles byte 499-500
        text.push_str("\n\nBody");
        let result = strip_title_prefix(&text, "Not Found");
        assert_eq!(result, text.as_str());
    }

    #[test]
    fn txt_writer_spaces_between_paragraphs() {
        // Block-level elements should produce line breaks in the plain text output,
        // not concatenated words.
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>First paragraph.</p><p>Second paragraph.</p>".into(),
            id: Some("ch1".into()),
        });
        let text = book_to_plain_text(&book);
        assert!(
            text.contains("First paragraph.\n\nSecond paragraph."),
            "Expected blank line between paragraphs in: {text}"
        );
    }

    #[test]
    fn txt_writer_skips_cover_chapter() {
        // A cover-only chapter (just an <img> tag with alt "Cover") should be
        // suppressed to avoid "Cover" appearing as the first line of output.
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: Some("Cover".into()),
            content: r#"<div><img src="cover.jpg" alt="Cover"/></div>"#.into(),
            id: Some("cover".into()),
        });
        book.add_chapter(&Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Once upon a time.</p>".into(),
            id: Some("ch1".into()),
        });

        let text = book_to_plain_text(&book);
        assert!(
            !text.starts_with("Cover"),
            "Cover artifact should not appear at the start of output: {text}"
        );
        assert!(text.contains("Chapter 1"));
        assert!(text.contains("Once upon a time."));
    }

    #[test]
    fn txt_writer_skips_empty_title_cover_chapter() {
        // A cover chapter with no title should also be suppressed.
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: None,
            content: r#"<img src="cover.jpg" alt="Cover"/>"#.into(),
            id: Some("cover".into()),
        });
        book.add_chapter(&Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Body text.</p>".into(),
            id: Some("ch1".into()),
        });

        let text = book_to_plain_text(&book);
        assert!(
            !text.contains("\nCover\n"),
            "Cover artifact should not appear in output: {text}"
        );
        assert!(text.contains("Chapter 1"));
    }

    #[test]
    fn txt_writer_keeps_real_chapter_named_cover() {
        // A chapter titled "Cover" with substantial body text should NOT be skipped.
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: Some("Cover".into()),
            content: "<p>This is a long description of the front artwork and its significance to the story.</p>".into(),
            id: Some("cover".into()),
        });
        let text = book_to_plain_text(&book);
        assert!(
            text.contains("long description of the front artwork"),
            "Substantial cover chapter content should be kept: {text}"
        );
    }

    #[test]
    fn txt_writer_suppresses_title_tag_text() {
        // Text inside <head><title>...</title></head> should not leak into output.
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: Some("Chapter 1".into()),
            content: r#"<?xml version="1.0"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Moby Dick; or The Whale | Project Gutenberg</title></head>
<body><p>Call me Ishmael.</p></body>
</html>"#
                .into(),
            id: Some("ch1".into()),
        });

        let text = book_to_plain_text(&book);
        assert!(
            !text.contains("Project Gutenberg"),
            "Title tag text should not leak into output: {text}"
        );
        assert!(
            text.contains("Call me Ishmael."),
            "Body text should still be present: {text}"
        );
    }
}
