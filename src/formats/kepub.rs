//! Kepub (Kobo EPUB) format reader and writer.
//!
//! Kepub is Kobo's EPUB variant that wraps text segments in `<span>` elements
//! for reading progress tracking. The reader delegates to the standard EPUB
//! reader (Kepub files are valid EPUBs). The writer adds Kobo span markup
//! before delegating to the EPUB writer.

use crate::domain::manifest::{Manifest, ManifestItem};
use crate::domain::{Book, FormatReader, FormatWriter};
use crate::error::Result;
use crate::formats::epub::{EpubReader, EpubWriter};
use std::io::{Read, Write};

/// Kepub format reader.
///
/// Delegates entirely to the EPUB reader since Kepub files are valid EPUBs.
/// Kobo-specific `<span class="koboSpan">` elements are left in the content
/// as-is (they are harmless HTML and will be stripped by transforms if needed).
#[derive(Default)]
pub struct KepubReader {
    inner: EpubReader,
}

impl KepubReader {
    pub fn new() -> Self {
        Self {
            inner: EpubReader::new(),
        }
    }
}

impl FormatReader for KepubReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        self.inner.read_book(reader)
    }
}

/// Kepub format writer.
///
/// Adds Kobo span markup to text content, then delegates to the EPUB writer.
/// Each text node in paragraph-level elements gets wrapped in:
/// `<span class="koboSpan" id="kobo.{paragraph}.{segment}">text</span>`
#[derive(Default)]
pub struct KepubWriter {
    inner: EpubWriter,
}

impl KepubWriter {
    pub fn new() -> Self {
        Self {
            inner: EpubWriter::new(),
        }
    }
}

impl FormatWriter for KepubWriter {
    fn write_book(&self, book: &Book, output: &mut dyn Write) -> Result<()> {
        let kobo_book = add_kobo_spans(book);
        self.inner.write_book(&kobo_book, output)
    }
}

/// Adds Kobo reading-progress spans to all text content documents in the book.
///
/// Builds a new `Book` by iterating the original's manifest items. Text items
/// that need Kobo spans are transformed directly from the original (avoiding
/// the clone-then-replace pattern). Binary items get a cheap `Arc` bump.
fn add_kobo_spans(book: &Book) -> Book {
    let mut manifest = Manifest::new();
    for item in book.manifest.iter() {
        if (item.media_type.contains("html") || item.media_type.contains("xml"))
            && let Some(text) = item.data.as_text()
        {
            // Build the item directly with wrapped text, avoiding a clone of
            // the original text content that would be immediately discarded.
            let mut modified =
                ManifestItem::new(&item.id, &item.href, &item.media_type)
                    .with_text(insert_kobo_spans(text));
            modified.fallback.clone_from(&item.fallback);
            modified.properties.clone_from(&item.properties);
            manifest.insert(modified);
        } else {
            manifest.insert(item.clone());
        }
    }

    Book {
        metadata: book.metadata.clone(),
        manifest,
        spine: book.spine.clone(),
        toc: book.toc.clone(),
        guide: book.guide.clone(),
    }
}

/// Inserts `<span class="koboSpan">` wrappers around text segments within
/// paragraph-level elements (`<p>`, `<h1>`–`<h6>`, `<li>`, `<blockquote>`,
/// `<div>`).
///
/// Uses a lightweight state machine rather than a full HTML parser.
fn insert_kobo_spans(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + html.len() / 4);
    let mut para_idx: u32 = 0;
    let mut seg_idx: u32;
    let mut pos = 0;
    let bytes = html.as_bytes();
    let len = bytes.len();

    // Track whether we're inside a paragraph-level element where spans should
    // be inserted.
    let mut in_block = false;
    let mut in_tag = false;

    while pos < len {
        if bytes[pos] == b'<' {
            // Find the end of this tag.
            let tag_start = pos;
            let tag_end = match html[pos..].find('>') {
                Some(e) => pos + e + 1,
                None => {
                    out.push_str(&html[pos..]);
                    break;
                },
            };

            let tag_content = &html[tag_start..tag_end];

            // Check for block-level open/close tags.
            if is_block_open_tag(tag_content) {
                in_block = true;
                para_idx += 1;
                seg_idx = 0;
                out.push_str(tag_content);
                pos = tag_end;

                // Now wrap text segments until the closing block tag.
                let close_tag = closing_tag_for(tag_content);
                loop {
                    if pos >= len {
                        break;
                    }

                    if bytes[pos] == b'<' {
                        let inner_end = match html[pos..].find('>') {
                            Some(e) => pos + e + 1,
                            None => len,
                        };
                        let inner_tag = &html[pos..inner_end];

                        if ascii_starts_with_ci(inner_tag, &close_tag) {
                            // Closing block tag — stop wrapping.
                            out.push_str(inner_tag);
                            pos = inner_end;
                            in_block = false;
                            break;
                        }

                        // Nested tag (e.g. <b>, <i>, <span>) — pass through.
                        out.push_str(inner_tag);
                        pos = inner_end;
                        in_tag = false;
                    } else {
                        // Text segment — wrap in koboSpan.
                        let text_start = pos;
                        while pos < len && bytes[pos] != b'<' {
                            pos += 1;
                        }
                        let text = &html[text_start..pos];
                        if !text.trim().is_empty() {
                            seg_idx += 1;
                            out.push_str("<span class=\"koboSpan\" id=\"kobo.");
                            push_u32(&mut out, para_idx);
                            out.push('.');
                            push_u32(&mut out, seg_idx);
                            out.push_str("\">");
                            out.push_str(text);
                            out.push_str("</span>");
                        } else {
                            out.push_str(text);
                        }
                    }
                }
                continue;
            }

            // Non-block tag — pass through.
            out.push_str(tag_content);
            pos = tag_end;
            in_tag = false;
        } else {
            // Text outside block elements — pass through without wrapping.
            let start = pos;
            while pos < len && bytes[pos] != b'<' {
                pos += 1;
            }
            out.push_str(&html[start..pos]);
        }
    }

    // Suppress unused variable warnings.
    let _ = in_block;
    let _ = in_tag;

    out
}

/// Case-insensitive ASCII prefix check.
fn ascii_starts_with_ci(s: &str, prefix: &str) -> bool {
    s.as_bytes()
        .get(..prefix.len())
        .is_some_and(|b| b.eq_ignore_ascii_case(prefix.as_bytes()))
}

/// Returns `true` if the tag opens a block-level element where Kobo spans
/// should be inserted.
fn is_block_open_tag(tag: &str) -> bool {
    let b = tag.as_bytes();
    let ci =
        |prefix: &[u8]| b.len() >= prefix.len() && b[..prefix.len()].eq_ignore_ascii_case(prefix);
    (ci(b"<p") && (b.len() < 3 || b[2] == b'>' || b[2] == b' '))
        || ci(b"<h1")
        || ci(b"<h2")
        || ci(b"<h3")
        || ci(b"<h4")
        || ci(b"<h5")
        || ci(b"<h6")
        || ci(b"<li")
        || ci(b"<blockquote")
}

/// Appends the decimal representation of a `u32` directly to a `String`
/// without allocating a temporary `String` (replaces `u32::to_string()`).
fn push_u32(s: &mut String, mut n: u32) {
    if n == 0 {
        s.push('0');
        return;
    }
    let mut buf = [0u8; 10]; // max digits for u32
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    // SAFETY: buf[i..] contains only ASCII digits, which is valid UTF-8.
    s.push_str(unsafe { std::str::from_utf8_unchecked(&buf[i..]) });
}

/// Returns the lowercase closing tag prefix for a given opening tag.
fn closing_tag_for(tag: &str) -> String {
    // Extract the tag name from something like "<P class='x'>"
    let name_end = tag[1..]
        .find(['>', ' ', '/'])
        .map(|i| i + 1)
        .unwrap_or(tag.len());
    let name = &tag[1..name_end];
    // Lowercase only the short tag name, not the full tag content.
    let mut close = String::with_capacity(2 + name.len());
    close.push_str("</");
    for &b in name.as_bytes() {
        close.push(b.to_ascii_lowercase() as char);
    }
    close
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;
    use std::io::Cursor;

    #[test]
    fn kepub_reader_delegates_to_epub() {
        // Build a minimal EPUB via the EPUB writer.
        let mut book = Book::new();
        book.metadata.title = Some("Kepub Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello Kobo</p>".into(),
            id: Some("ch1".into()),
        });

        let mut epub_buf = Vec::new();
        EpubWriter::new().write_book(&book, &mut epub_buf).unwrap();

        // Read it back via KepubReader.
        let mut cursor = Cursor::new(epub_buf);
        let decoded = KepubReader::new().read_book(&mut cursor).unwrap();
        assert_eq!(decoded.metadata.title.as_deref(), Some("Kepub Test"));
        assert!(!decoded.chapters().is_empty());
    }

    #[test]
    fn kepub_writer_produces_valid_epub() {
        let mut book = Book::new();
        book.metadata.title = Some("Kobo Book".into());
        book.add_chapter(Chapter {
            title: Some("Chapter One".into()),
            content: "<p>Some text here.</p>".into(),
            id: Some("ch1".into()),
        });

        // Write via KepubWriter.
        let mut buf = Vec::new();
        KepubWriter::new().write_book(&book, &mut buf).unwrap();

        // Should be readable as a standard EPUB.
        let mut cursor = Cursor::new(buf);
        let decoded = EpubReader::new().read_book(&mut cursor).unwrap();
        assert_eq!(decoded.metadata.title.as_deref(), Some("Kobo Book"));
    }

    #[test]
    fn kobo_spans_wrap_text() {
        let html = "<p>Hello world</p>";
        let result = insert_kobo_spans(html);
        assert!(result.contains("koboSpan"));
        assert!(result.contains("Hello world"));
        assert!(result.contains("kobo.1.1"));
    }

    #[test]
    fn kobo_spans_skip_empty_text() {
        let html = "<p>  </p>";
        let result = insert_kobo_spans(html);
        // Whitespace-only text should not get a span.
        assert!(!result.contains("koboSpan"));
    }

    #[test]
    fn kobo_spans_handle_inline_tags() {
        let html = "<p><b>Bold</b> and <i>italic</i></p>";
        let result = insert_kobo_spans(html);
        assert!(result.contains("koboSpan"));
        assert!(result.contains("<b>"));
        assert!(result.contains("<i>"));
    }

    #[test]
    fn kobo_spans_increment_paragraph_counter() {
        let html = "<p>First</p><p>Second</p>";
        let result = insert_kobo_spans(html);
        assert!(result.contains("kobo.1.1"));
        assert!(result.contains("kobo.2.1"));
    }

    #[test]
    fn closing_tag_extraction() {
        assert_eq!(closing_tag_for("<p>"), "</p");
        assert_eq!(closing_tag_for("<p class=\"x\">"), "</p");
        assert_eq!(closing_tag_for("<blockquote>"), "</blockquote");
        assert_eq!(closing_tag_for("<h2>"), "</h2");
    }

    #[test]
    fn kepub_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("Round Trip".into());
        book.metadata.authors.push("Author".into());
        book.add_chapter(Chapter {
            title: Some("Chapter".into()),
            content: "<p>Content here</p>".into(),
            id: Some("ch1".into()),
        });

        let mut buf = Vec::new();
        KepubWriter::new().write_book(&book, &mut buf).unwrap();

        let mut cursor = Cursor::new(buf);
        let decoded = KepubReader::new().read_book(&mut cursor).unwrap();
        assert_eq!(decoded.metadata.title.as_deref(), Some("Round Trip"));
        assert!(decoded.metadata.authors.contains(&"Author".to_string()));
    }
}
