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

/// Extracts the text of the first heading (h1-h6) found in HTML content.
///
/// Uses a single-pass byte-level scan so the actual first heading in document
/// order is returned, regardless of its level. Previous versions scanned for
/// h1, then h2, then h3 separately, which could miss an h2 that appeared
/// before any h1.
fn extract_first_heading(html: &str) -> Option<String> {
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i + 3 < len {
        if bytes[i] == b'<'
            && (bytes[i + 1] == b'h' || bytes[i + 1] == b'H')
            && bytes[i + 2] >= b'1'
            && bytes[i + 2] <= b'6'
            && (bytes[i + 3] == b'>'
                || bytes[i + 3] == b' '
                || bytes[i + 3] == b'\t'
                || bytes[i + 3] == b'\n'
                || bytes[i + 3] == b'\r')
        {
            let level = bytes[i + 2] - b'0';
            // Find closing '>' of the opening tag.
            let tag_end = match html[i..].find('>') {
                Some(pos) => i + pos + 1,
                None => {
                    i += 1;
                    continue;
                }
            };
            // Build the closing tag and search case-insensitively.
            let close_tag = [b'<', b'/', b'h', b'0' + level, b'>'];
            let close_pos = match crate::formats::common::text_utils::find_case_insensitive(
                &bytes[tag_end..],
                &close_tag,
            ) {
                Some(pos) => tag_end + pos,
                None => {
                    i += 1;
                    continue;
                }
            };
            let heading_html = &html[tag_end..close_pos];
            let heading_text = strip_inner_tags(heading_html);
            let text = heading_text.trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
        i += 1;
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
            // Strip fragment identifiers (e.g. "chapter1.html#section1" → "chapter1.html")
            // so that TOC entries with anchors still match bare spine/manifest hrefs.
            let base_href = strip_fragment(&item.href);
            out.insert(base_href);
            if !item.children.is_empty() {
                stack.push(&item.children);
            }
        }
    }
}

/// Returns the href with any `#fragment` removed.
fn strip_fragment(href: &str) -> String {
    match href.find('#') {
        Some(pos) => href[..pos].to_string(),
        None => href.to_string(),
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
    fn picks_first_heading_in_document_order() {
        // An h2 before an h1 should return the h2 (document order).
        let html = "<h2>First</h2><h1>Second</h1>";
        assert_eq!(extract_first_heading(html), Some("First".into()));
    }

    #[test]
    fn handles_h4_through_h6() {
        assert_eq!(
            extract_first_heading("<h4>Level 4</h4>"),
            Some("Level 4".into())
        );
        assert_eq!(
            extract_first_heading("<h6>Level 6</h6>"),
            Some("Level 6".into())
        );
    }

    #[test]
    fn case_insensitive_heading_tags() {
        let html = "<H1>Upper</H1><p>text</p>";
        assert_eq!(extract_first_heading(html), Some("Upper".into()));
    }

    #[test]
    fn generator_adds_missing_toc_entries() {
        let mut book = Book::new();
        book.add_chapter(Chapter {
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
        book.add_chapter(Chapter {
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
        book.add_chapter(Chapter {
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
    fn strip_fragment_removes_anchor() {
        assert_eq!(strip_fragment("chapter1.html#section2"), "chapter1.html");
        assert_eq!(strip_fragment("chapter1.html"), "chapter1.html");
        assert_eq!(strip_fragment("#only-fragment"), "");
    }

    #[test]
    fn collect_toc_hrefs_strips_fragments() {
        let items = vec![
            TocItem::new("Ch 1", "chapter1.html#sec1"),
            TocItem::new("Ch 2", "chapter2.html"),
        ];
        let hrefs = collect_toc_hrefs(&items);
        // Fragment should be stripped, so bare href matches.
        assert!(hrefs.contains("chapter1.html"));
        assert!(hrefs.contains("chapter2.html"));
        // The original href with fragment should NOT be in the set.
        assert!(!hrefs.contains("chapter1.html#sec1"));
    }

    #[test]
    fn generator_no_duplicates_when_toc_has_fragment_hrefs() {
        // Simulates the EPUB round-trip scenario: NCX entries have fragment hrefs
        // (e.g. "ch1.xhtml#anchor") but spine/manifest items have bare hrefs
        // (e.g. "ch1.xhtml"). Without fragment stripping, the generator would
        // add duplicate entries for already-covered chapters.
        let mut book = Book::new();
        book.add_chapter(Chapter {
            title: None,
            content: "<h1>Chapter One</h1><p>Content</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: None,
            content: "<h1>Chapter Two</h1><p>Content</p>".into(),
            id: Some("ch2".into()),
        });

        // Manually set TOC entries with fragment hrefs (as NCX would produce).
        book.toc = vec![
            TocItem::new("Chapter One", "ch1.xhtml#anchor1"),
            TocItem::new("Chapter Two", "ch2.xhtml#anchor2"),
        ];

        let toc_gen = TocGenerator;
        let result = toc_gen.apply(book).unwrap();

        // No new entries should be added since the chapters are already covered.
        assert_eq!(
            result.toc.len(),
            2,
            "TocGenerator should not add duplicates when existing TOC hrefs have fragments; got {:?}",
            result.toc.iter().map(|t| &t.title).collect::<Vec<_>>()
        );
    }

    #[test]
    fn generator_skips_cover_page_image_only() {
        let mut book = Book::new();
        // Simulate a cover page that is just an image with no heading.
        book.add_chapter(Chapter {
            title: None,
            content: "<div><img src=\"cover.jpg\" alt=\"Cover\" /></div>".into(),
            id: Some("cover".into()),
        });
        // Add a real chapter with a heading.
        book.add_chapter(Chapter {
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
