//! Generates or rebuilds the table of contents from spine content.

use std::collections::HashSet;

use crate::domain::Book;
use crate::domain::toc::TocItem;
use crate::domain::traits::Transform;
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

    fn apply(&self, book: Book) -> Result<Book> {
        let mut result = book;

        // Collect hrefs already in the TOC.
        let existing_hrefs = collect_toc_hrefs(&result.toc);

        // Add entries for spine items that aren't already in the TOC.
        let mut new_entries = Vec::new();
        for spine_item in result.spine.iter() {
            if let Some(item) = result.manifest.get(&spine_item.manifest_id) {
                if existing_hrefs.contains(&item.href) {
                    continue;
                }

                // Only add a TOC entry if the content has a real heading.
                // Skip items without headings (e.g. cover pages with just an image).
                if let Some(title) = item.data.as_text().and_then(extract_first_heading) {
                    new_entries.push(TocItem::new(&title, &item.href));
                }
            }
        }

        result.toc.extend(new_entries);
        Ok(result)
    }
}

/// Extracts the text of the first heading (h1-h3) found in HTML content.
///
/// Uses byte-level scanning to avoid `format!` allocations for tag patterns
/// on each heading level.
fn extract_first_heading(html: &str) -> Option<String> {
    let bytes = html.as_bytes();
    // Pre-computed open/close patterns for h1, h2, h3.
    const OPEN_TAGS: [&[u8]; 3] = [b"<h1", b"<h2", b"<h3"];
    const CLOSE_TAGS: [&str; 3] = ["</h1>", "</h2>", "</h3>"];

    for (open_tag, close_tag) in OPEN_TAGS.iter().zip(CLOSE_TAGS.iter()) {
        if let Some(start) = memchr::memmem::find(bytes, open_tag) {
            let content_start = memchr::memchr(b'>', &bytes[start..])? + start + 1;
            let content_end = html[content_start..].find(close_tag)? + content_start;
            let inner = &html[content_start..content_end];
            let text = strip_inner_tags(inner).trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

/// Strips HTML tags from inner content using `memchr` for fast scanning.
fn strip_inner_tags(html: &str) -> String {
    let bytes = html.as_bytes();
    // Fast path: no tags at all.
    if memchr::memchr(b'<', bytes).is_none() {
        return html.to_string();
    }

    let mut result = String::with_capacity(html.len());
    let mut pos = 0;

    while pos < bytes.len() {
        match memchr::memchr(b'<', &bytes[pos..]) {
            Some(offset) => {
                // Copy text before the tag.
                if offset > 0 {
                    result.push_str(&html[pos..pos + offset]);
                }
                let tag_start = pos + offset;
                // Find the closing '>'.
                match memchr::memchr(b'>', &bytes[tag_start..]) {
                    Some(end_offset) => {
                        pos = tag_start + end_offset + 1;
                    },
                    None => break, // Unclosed tag -- stop.
                }
            },
            None => {
                result.push_str(&html[pos..]);
                break;
            },
        }
    }
    result
}

/// Recursively collects all hrefs from a TOC tree into a set for O(1) lookup.
fn collect_toc_hrefs(items: &[TocItem]) -> HashSet<String> {
    let mut hrefs = HashSet::new();
    collect_toc_hrefs_into(items, &mut hrefs);
    hrefs
}

fn collect_toc_hrefs_into(items: &[TocItem], out: &mut HashSet<String>) {
    let mut stack: Vec<&[TocItem]> = vec![items];
    while let Some(current) = stack.pop() {
        for item in current {
            out.insert(item.href.clone());
            if !item.children.is_empty() {
                stack.push(&item.children);
            }
        }
    }
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
        let result = toc_gen.apply(book).unwrap();

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
        let result = toc_gen.apply(book).unwrap();

        // Entry already existed, so nothing new should be added.
        assert_eq!(result.toc.len(), 1);
        assert_eq!(result.toc[0].title, "Already There");
    }

    #[test]
    fn generator_skips_items_without_headings() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>No heading here</p>".into(),
            id: Some("ch1".into()),
        });
        book.toc.clear();

        let toc_gen = TocGenerator;
        let result = toc_gen.apply(book).unwrap();

        // Items without headings should be skipped entirely.
        assert_eq!(result.toc.len(), 0);
    }

    #[test]
    fn generator_skips_cover_page_image_only() {
        let mut book = Book::new();
        // Simulate a cover page that is just an image with no heading.
        book.add_chapter(&Chapter {
            title: None,
            content: "<div><img src=\"cover.jpg\" alt=\"Cover\" /></div>".into(),
            id: Some("cover".into()),
        });
        // Add a real chapter with a heading.
        book.add_chapter(&Chapter {
            title: None,
            content: "<h1>Introduction</h1><p>Welcome</p>".into(),
            id: Some("ch1".into()),
        });
        book.toc.clear();

        let toc_gen = TocGenerator;
        let result = toc_gen.apply(book).unwrap();

        // Only the chapter with a real heading should appear.
        assert_eq!(result.toc.len(), 1);
        assert_eq!(result.toc[0].title, "Introduction");
    }
}
