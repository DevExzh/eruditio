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

    fn apply(&self, book: &Book) -> Result<Book> {
        let referenced = collect_referenced_ids(book);

        let mut result = book.clone();
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
        if let Some(item) = book.manifest.iter().find(|i| i.href == guide_ref.href) {
            ids.insert(item.id.clone());
        }
    }

    // TOC references (resolve href → manifest ID).
    collect_toc_refs(&book.toc, book, &mut ids);

    // Content references: scan XHTML content for hrefs to other manifest items.
    let content_ids: Vec<String> = ids.iter().cloned().collect();
    for id in &content_ids {
        if let Some(item) = book.manifest.get(id)
            && let Some(text) = item.data.as_text()
        {
            collect_href_references(text, book, &mut ids);
        }
    }

    ids
}

/// Recursively collects manifest IDs referenced by TOC entries.
fn collect_toc_refs(
    items: &[crate::domain::toc::TocItem],
    book: &Book,
    ids: &mut HashSet<String>,
) {
    for toc_item in items {
        // Strip fragment from href for matching.
        let href = toc_item.href.split('#').next().unwrap_or(&toc_item.href);
        if let Some(item) = book.manifest.iter().find(|i| i.href == href) {
            ids.insert(item.id.clone());
        }
        collect_toc_refs(&toc_item.children, book, ids);
    }
}

/// Scans HTML/XHTML text for href and src attributes pointing to manifest items.
fn collect_href_references(text: &str, book: &Book, ids: &mut HashSet<String>) {
    // Simple attribute extraction — look for href="..." and src="..." patterns.
    for attr in &["href=\"", "src=\""] {
        let mut search_from = 0;
        while let Some(start) = text[search_from..].find(attr) {
            let value_start = search_from + start + attr.len();
            if let Some(end) = text[value_start..].find('"') {
                let value = &text[value_start..value_start + end];
                // Strip fragment.
                let path = value.split('#').next().unwrap_or(value);
                // Match against manifest hrefs.
                if let Some(item) = book.manifest.iter().find(|i| {
                    i.href == path || i.href.ends_with(path)
                }) {
                    ids.insert(item.id.clone());
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
        let result = trimmer.apply(&book).unwrap();

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
        let result = trimmer.apply(&book).unwrap();

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
        let result = trimmer.apply(&book).unwrap();

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
        let result = trimmer.apply(&book).unwrap();

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
        let _result = trimmer.apply(&book).unwrap();

        // Original still has the orphan.
        assert!(book.manifest.get("orphan").is_some());
    }
}
