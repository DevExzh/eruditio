//! Detects chapter structure from heading tags in content.

use crate::domain::Book;
use crate::domain::toc::TocItem;
use crate::domain::traits::Transform;
use crate::error::Result;

/// Detects chapter structure by scanning content for heading tags (h1-h3).
///
/// If the book's TOC is empty, this transform builds one from headings found
/// in the spine content documents. Existing TOC entries are preserved.
pub struct StructureDetector;

impl Transform for StructureDetector {
    fn name(&self) -> &str {
        "structure_detector"
    }

    fn apply(&self, book: Book) -> Result<Book> {
        // Only generate structure if TOC is empty.
        if !book.toc.is_empty() {
            return Ok(book);
        }

        let mut result = book;
        let mut toc_entries = Vec::new();

        for spine_item in result.spine.iter() {
            if let Some(item) = result.manifest.get(&spine_item.manifest_id)
                && let Some(text) = item.data.as_text()
            {
                let headings = extract_headings(text);
                for (title, _level) in headings {
                    toc_entries.push(TocItem::new(&title, &item.href));
                }
            }
        }

        result.toc = toc_entries;
        Ok(result)
    }
}

/// Extracts heading text and level from HTML content.
///
/// Performs a single linear scan of the document so headings are returned
/// in document order (not grouped by level). Uses `memchr` for byte-level
/// scanning of `<h` tags and pre-computed close-tag patterns to avoid
/// allocations in the inner loop.
fn extract_headings(html: &str) -> Vec<(String, u8)> {
    let bytes = html.as_bytes();
    let mut headings = Vec::new();
    let mut search_from = 0;

    // Pre-computed close tags to avoid format! allocation per heading.
    const CLOSE_TAGS: [&str; 3] = ["</h1>", "</h2>", "</h3>"];

    while search_from < bytes.len() {
        // Find the next "<h" using memchr for the '<', then verify 'h'.
        let pos = match memchr::memchr(b'<', &bytes[search_from..]) {
            Some(p) => search_from + p,
            None => break,
        };
        let after_lt = pos + 1;
        if after_lt >= bytes.len() || bytes[after_lt] != b'h' {
            search_from = pos + 1;
            continue;
        }

        let after_h = pos + 2;
        if after_h >= bytes.len() {
            break;
        }

        let level_byte = bytes[after_h];
        if !matches!(level_byte, b'1' | b'2' | b'3') {
            search_from = pos + 1;
            continue;
        }
        let level = level_byte - b'0';
        let close_tag = CLOSE_TAGS[(level - 1) as usize];

        // Find the end of the opening tag using memchr.
        let content_start = match memchr::memchr(b'>', &bytes[pos..]) {
            Some(gt) => pos + gt + 1,
            None => break,
        };

        // Find the closing tag.
        let content_end = match html[content_start..].find(close_tag) {
            Some(ct) => content_start + ct,
            None => {
                search_from = content_start;
                continue;
            },
        };

        let inner = &html[content_start..content_end];
        let text = strip_tags(inner).trim().to_string();

        if !text.is_empty() {
            headings.push((text, level));
        }

        search_from = content_end + close_tag.len();
    }

    headings
}

/// Strips HTML tags from a string, returning only text content.
fn strip_tags(html: &str) -> String {
    crate::formats::common::text_utils::strip_tags(html).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;

    #[test]
    fn extracts_h1_headings() {
        let html = "<h1>Chapter One</h1><p>Text</p><h1>Chapter Two</h1>";
        let headings = extract_headings(html);
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0], ("Chapter One".into(), 1));
        assert_eq!(headings[1], ("Chapter Two".into(), 1));
    }

    #[test]
    fn extracts_nested_heading_text() {
        let html = "<h2><span>Bold</span> Heading</h2>";
        let headings = extract_headings(html);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].0, "Bold Heading");
    }

    #[test]
    fn ignores_empty_headings() {
        let html = "<h1></h1><h1>  </h1><h1>Real</h1>";
        let headings = extract_headings(html);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].0, "Real");
    }

    #[test]
    fn detector_builds_toc_from_headings() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: None,
            content: "<h1>Introduction</h1><p>Hello</p>".into(),
            id: Some("ch1".into()),
        });
        // Clear the TOC so structure detection kicks in.
        book.toc.clear();

        let detector = StructureDetector;
        let result = detector.apply(book).unwrap();

        assert_eq!(result.toc.len(), 1);
        assert_eq!(result.toc[0].title, "Introduction");
    }

    #[test]
    fn detector_preserves_existing_toc() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: Some("Existing".into()),
            content: "<h1>Detected</h1><p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        let detector = StructureDetector;
        let result = detector.apply(book).unwrap();

        // TOC already had entries from add_chapter, so it should be unchanged.
        assert_eq!(result.toc.len(), 1);
        assert_eq!(result.toc[0].title, "Existing");
    }
}
