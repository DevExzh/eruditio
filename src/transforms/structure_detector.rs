//! Detects chapter structure from heading tags in content.

use crate::domain::Book;
use crate::domain::traits::Transform;
use crate::domain::toc::TocItem;
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

    fn apply(&self, book: &Book) -> Result<Book> {
        // Only generate structure if TOC is empty.
        if !book.toc.is_empty() {
            return Ok(book.clone());
        }

        let mut result = book.clone();
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
/// Finds `<h1>` through `<h3>` tags and returns their text content
/// stripped of inner HTML. Returns (title, level) pairs.
fn extract_headings(html: &str) -> Vec<(String, u8)> {
    let mut headings = Vec::new();

    for level in 1..=3u8 {
        let open_tag = format!("<h{}", level);
        let close_tag = format!("</h{}>", level);

        let mut search_from = 0;
        while let Some(start) = html[search_from..].find(&open_tag) {
            let abs_start = search_from + start;

            // Find the end of the opening tag.
            let content_start = match html[abs_start..].find('>') {
                Some(pos) => abs_start + pos + 1,
                None => break,
            };

            // Find the closing tag.
            let content_end = match html[content_start..].find(&close_tag) {
                Some(pos) => content_start + pos,
                None => break,
            };

            let inner = &html[content_start..content_end];
            let text = strip_tags(inner).trim().to_string();

            if !text.is_empty() {
                headings.push((text, level));
            }

            search_from = content_end + close_tag.len();
        }
    }

    headings
}

/// Strips HTML tags from a string, returning only text content.
fn strip_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }

    result
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
        let result = detector.apply(&book).unwrap();

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
        let result = detector.apply(&book).unwrap();

        // TOC already had entries from add_chapter, so it should be unchanged.
        assert_eq!(result.toc.len(), 1);
        assert_eq!(result.toc[0].title, "Existing");
    }
}
