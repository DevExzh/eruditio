//! Removes unreferenced resources from the manifest.

use crate::domain::Book;
use crate::domain::traits::Transform;
use crate::error::Result;
use std::collections::HashSet;

/// Removes manifest items that are not referenced by the spine, TOC,
/// guide, cover metadata, or other content documents.
///
/// This reduces output file size by dropping orphaned resources.
pub struct ManifestTrimmer;

impl Transform for ManifestTrimmer {
    fn name(&self) -> &str {
        "manifest_trimmer"
    }

    fn apply(&self, book: Book) -> Result<Book> {
        let referenced = collect_referenced_ids(&book);

        let mut result = book;
        let all_ids: Vec<String> = result.manifest.iter().map(|item| item.id.clone()).collect();

        for id in &all_ids {
            if !referenced.contains(id.as_str()) {
                result.manifest.remove(id);
            }
        }

        Ok(result)
    }
}

/// Collects all manifest IDs that are referenced somewhere in the book.
fn collect_referenced_ids(book: &Book) -> HashSet<String> {
    let mut ids = HashSet::new();

    // Build a href→id index once, replacing O(n) linear scans per lookup.
    let href_to_id: std::collections::HashMap<&str, &str> = book
        .manifest
        .iter()
        .map(|item| (item.href.as_str(), item.id.as_str()))
        .collect();

    // Spine references.
    for spine_item in book.spine.iter() {
        ids.insert(spine_item.manifest_id.clone());
    }

    // Cover image reference.
    if let Some(ref cover_id) = book.metadata.cover_image_id {
        ids.insert(cover_id.clone());
    }

    // Guide references (resolve href → manifest ID).
    for guide_ref in &book.guide.references {
        if let Some(&id) = href_to_id.get(guide_ref.href.as_str()) {
            ids.insert(id.to_string());
        }
    }

    // TOC references (resolve href → manifest ID).
    collect_toc_refs(&book.toc, &href_to_id, &mut ids);

    // Content references: scan XHTML content for hrefs to other manifest items.
    let content_ids: Vec<String> = ids.iter().cloned().collect();
    for id in &content_ids {
        if let Some(item) = book.manifest.get(id)
            && let Some(text) = item.data.as_text()
        {
            collect_href_references(text, &href_to_id, book, &mut ids);
        }
    }

    ids
}

/// Recursively collects manifest IDs referenced by TOC entries.
fn collect_toc_refs(
    items: &[crate::domain::toc::TocItem],
    href_to_id: &std::collections::HashMap<&str, &str>,
    ids: &mut HashSet<String>,
) {
    for toc_item in items {
        // Strip fragment from href for matching.
        let href = toc_item.href.split('#').next().unwrap_or(&toc_item.href);
        if let Some(&id) = href_to_id.get(href) {
            ids.insert(id.to_string());
        }
        collect_toc_refs(&toc_item.children, href_to_id, ids);
    }
}

/// Scans HTML/XHTML text for href and src attributes pointing to manifest items.
///
/// Uses byte-level scanning via `memchr` to avoid the overhead of Rust's
/// `str::pattern` machinery on every iteration.
fn collect_href_references(
    text: &str,
    href_to_id: &std::collections::HashMap<&str, &str>,
    book: &Book,
    ids: &mut HashSet<String>,
) {
    let bytes = text.as_bytes();
    // Attribute patterns to search for, with their byte representations.
    let patterns: &[&[u8]] = &[b"href=\"", b"src=\""];

    for &pattern in patterns {
        let pat_len = pattern.len();
        let mut search_from = 0;
        while search_from + pat_len <= bytes.len() {
            // Use memchr to find the first byte of the pattern quickly,
            // then verify the remaining bytes.
            let haystack = &bytes[search_from..];
            let start = match memchr::memchr(pattern[0], haystack) {
                Some(pos) => pos,
                None => break,
            };
            // Check if the full pattern matches at this position.
            let abs_start = search_from + start;
            if abs_start + pat_len > bytes.len()
                || bytes[abs_start..abs_start + pat_len] != *pattern
            {
                search_from = abs_start + 1;
                continue;
            }
            let value_start = abs_start + pat_len;
            // Find closing quote using memchr (single byte search).
            if let Some(end) = memchr::memchr(b'"', &bytes[value_start..]) {
                let value = &text[value_start..value_start + end];
                // Strip fragment.
                let path = value.split('#').next().unwrap_or(value);
                // Fast O(1) lookup by exact href match.
                if let Some(&id) = href_to_id.get(path) {
                    ids.insert(id.to_string());
                } else {
                    // Fallback: suffix match for relative paths.
                    if let Some(item) = book.manifest.iter().find(|i| i.href.ends_with(path)) {
                        ids.insert(item.id.clone());
                    }
                }
                search_from = value_start + end;
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;

    #[test]
    fn keeps_spine_items() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: Some("Ch".into()),
            content: "<p>text</p>".into(),
            id: Some("ch1".into()),
        });

        let trimmer = ManifestTrimmer;
        let result = trimmer.apply(book).unwrap();

        assert!(result.manifest.get("ch1").is_some());
    }

    #[test]
    fn removes_unreferenced_resources() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: Some("Ch".into()),
            content: "<p>text</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_resource("orphan", "orphan.css", b"body{}".to_vec(), "text/css");

        let trimmer = ManifestTrimmer;
        let result = trimmer.apply(book).unwrap();

        assert!(result.manifest.get("ch1").is_some());
        assert!(result.manifest.get("orphan").is_none());
    }

    #[test]
    fn keeps_cover_image() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>text</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_resource("cover", "cover.jpg", vec![0xFF, 0xD8], "image/jpeg");
        book.metadata.cover_image_id = Some("cover".into());

        let trimmer = ManifestTrimmer;
        let result = trimmer.apply(book).unwrap();

        assert!(result.manifest.get("cover").is_some());
    }

    #[test]
    fn keeps_referenced_image() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: None,
            content: "<p><img src=\"photo.jpg\" /></p>".into(),
            id: Some("ch1".into()),
        });
        book.add_resource("photo", "photo.jpg", vec![0xFF, 0xD8], "image/jpeg");

        let trimmer = ManifestTrimmer;
        let result = trimmer.apply(book).unwrap();

        assert!(result.manifest.get("photo").is_some());
    }

    #[test]
    fn does_not_mutate_original() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>text</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_resource("orphan", "orphan.css", b"body{}".to_vec(), "text/css");

        let trimmer = ManifestTrimmer;
        let _result = trimmer.apply(book.clone()).unwrap();

        // Original still has the orphan.
        assert!(book.manifest.get("orphan").is_some());
    }
}
