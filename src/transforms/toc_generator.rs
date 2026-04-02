//! Generates or rebuilds the table of contents from spine content.

use crate::domain::Book;
use crate::domain::traits::Transform;
use crate::domain::toc::TocItem;
use crate::error::Result;

/// Generates a table of contents from the book's spine documents.
///
/// If the book already has TOC entries, this enhances them with any
/// missing chapters. If no TOC exists, it creates entries from spine
/// document titles or first headings.
pub struct TocGenerator;

impl Transform for TocGenerator {
    fn name(&self) -> &str {
        "toc_generator"
    }

    fn apply(&self, book: &Book) -> Result<Book> {
        let mut result = book.clone();

        // Collect hrefs already in the TOC.
        let existing_hrefs: Vec<String> = collect_toc_hrefs(&result.toc);

        // Add entries for spine items that aren't already in the TOC.
        let mut new_entries = Vec::new();
        for (index, spine_item) in result.spine.iter().enumerate() {
            if let Some(item) = result.manifest.get(&spine_item.manifest_id) {
                if existing_hrefs.contains(&item.href) {
                    continue;
                }

                // Try to extract a title from the content.
                let title = item
                    .data
                    .as_text()
                    .and_then(extract_first_heading)
                    .unwrap_or_else(|| format!("Chapter {}", index + 1));

                new_entries.push(TocItem::new(&title, &item.href));
            }
        }

        result.toc.extend(new_entries);
        Ok(result)
    }
}

/// Extracts the text of the first heading (h1-h3) found in HTML content.
fn extract_first_heading(html: &str) -> Option<String> {
    for level in 1..=3u8 {
        let open_tag = format!("<h{}", level);
        let close_tag = format!("</h{}>", level);

        if let Some(start) = html.find(&open_tag) {
            let content_start = html[start..].find('>')? + start + 1;
            let content_end = html[content_start..].find(&close_tag)? + content_start;
            let inner = &html[content_start..content_end];
            let text = strip_inner_tags(inner).trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

/// Strips HTML tags from inner content.
fn strip_inner_tags(html: &str) -> String {
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

/// Recursively collects all hrefs from a TOC tree.
fn collect_toc_hrefs(items: &[TocItem]) -> Vec<String> {
    let mut hrefs = Vec::new();
    for item in items {
        hrefs.push(item.href.clone());
        hrefs.extend(collect_toc_hrefs(&item.children));
    }
    hrefs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;

    #[test]
    fn extracts_h1_title() {
        let html = "<h1>My Title</h1><p>body</p>";
        assert_eq!(extract_first_heading(html), Some("My Title".into()));
    }

    #[test]
    fn falls_back_to_h2() {
        let html = "<h2>Subtitle</h2><p>body</p>";
        assert_eq!(extract_first_heading(html), Some("Subtitle".into()));
    }

    #[test]
    fn returns_none_for_no_heading() {
        let html = "<p>Just a paragraph</p>";
        assert_eq!(extract_first_heading(html), None);
    }

    #[test]
    fn generator_adds_missing_toc_entries() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: None,
            content: "<h1>Intro</h1><p>Hello</p>".into(),
            id: Some("ch1".into()),
        });
        // Clear TOC to simulate a book without one.
        book.toc.clear();

        let toc_gen = TocGenerator;
        let result = toc_gen.apply(&book).unwrap();

        assert_eq!(result.toc.len(), 1);
        assert_eq!(result.toc[0].title, "Intro");
    }

    #[test]
    fn generator_preserves_existing_entries() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: Some("Already There".into()),
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        let toc_gen = TocGenerator;
        let result = toc_gen.apply(&book).unwrap();

        // Entry already existed, so nothing new should be added.
        assert_eq!(result.toc.len(), 1);
        assert_eq!(result.toc[0].title, "Already There");
    }

    #[test]
    fn generator_uses_fallback_title() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>No heading here</p>".into(),
            id: Some("ch1".into()),
        });
        book.toc.clear();

        let toc_gen = TocGenerator;
        let result = toc_gen.apply(&book).unwrap();

        assert_eq!(result.toc.len(), 1);
        assert_eq!(result.toc[0].title, "Chapter 1");
    }
}
