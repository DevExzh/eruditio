//! Detects and sets the cover image in the book.

use crate::domain::Book;
use crate::domain::traits::Transform;
use crate::error::Result;
use crate::formats::common::text_utils::contains_ascii_ci;

/// Detects and assigns the cover image in the book's metadata and guide.
///
/// Scans the manifest for likely cover images by name convention
/// (e.g., "cover", "cover-image") and media type (image/*). If found
/// and not already set, updates `metadata.cover_image_id` and adds
/// a guide reference.
pub struct CoverHandler;

impl Transform for CoverHandler {
    fn name(&self) -> &str {
        "cover_handler"
    }

    fn apply(&self, book: Book) -> Result<Book> {
        // If cover is already set, nothing to do.
        if book.metadata.cover_image_id.is_some() {
            return Ok(book);
        }

        let mut result = book;

        // Strategy 1: Look for an image whose ID contains "cover".
        if let Some(cover_id) = find_cover_by_id(&result) {
            result.metadata.cover_image_id = Some(cover_id.clone());
            add_cover_guide_ref(&mut result, &cover_id);
            return Ok(result);
        }

        // Strategy 2: Look for an image whose href contains "cover".
        if let Some(cover_id) = find_cover_by_href(&result) {
            result.metadata.cover_image_id = Some(cover_id.clone());
            add_cover_guide_ref(&mut result, &cover_id);
            return Ok(result);
        }

        // Strategy 3: Use the first image in the manifest as a fallback.
        if let Some(cover_id) = find_first_image(&result) {
            result.metadata.cover_image_id = Some(cover_id.clone());
            add_cover_guide_ref(&mut result, &cover_id);
            return Ok(result);
        }

        Ok(result)
    }
}

/// Finds a manifest item whose ID contains "cover" and is an image.
fn find_cover_by_id(book: &Book) -> Option<String> {
    book.manifest
        .iter()
        .find(|item| item.media_type.starts_with("image/") && contains_ascii_ci(&item.id, "cover"))
        .map(|item| item.id.clone())
}

/// Finds a manifest item whose href contains "cover" and is an image.
fn find_cover_by_href(book: &Book) -> Option<String> {
    book.manifest
        .iter()
        .find(|item| {
            item.media_type.starts_with("image/") && contains_ascii_ci(&item.href, "cover")
        })
        .map(|item| item.id.clone())
}

/// Returns the ID of the first image resource in the manifest.
fn find_first_image(book: &Book) -> Option<String> {
    book.manifest
        .iter()
        .find(|item| item.media_type.starts_with("image/"))
        .map(|item| item.id.clone())
}

/// Adds a guide reference for the cover image.
fn add_cover_guide_ref(book: &mut Book, cover_id: &str) {
    if let Some(item) = book.manifest.get(cover_id) {
        use crate::domain::guide::{GuideReference, GuideType};
        let guide_ref = GuideReference {
            ref_type: GuideType::Cover,
            title: "Cover".into(),
            href: item.href.clone(),
        };
        book.guide.push(guide_ref);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;

    #[test]
    fn detects_cover_by_id() {
        let mut book = Book::new();
        book.add_chapter(Chapter {
            title: None,
            content: "<p>text</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_resource(
            "cover-image",
            "images/cover.jpg",
            vec![0xFF, 0xD8],
            "image/jpeg",
        );

        let handler = CoverHandler;
        let result = handler.apply(book.clone()).unwrap();

        assert_eq!(
            result.metadata.cover_image_id.as_deref(),
            Some("cover-image")
        );
    }

    #[test]
    fn detects_cover_by_href() {
        let mut book = Book::new();
        book.add_resource("img1", "images/cover.png", vec![0x89, 0x50], "image/png");

        let handler = CoverHandler;
        let result = handler.apply(book).unwrap();

        assert_eq!(result.metadata.cover_image_id.as_deref(), Some("img1"));
    }

    #[test]
    fn preserves_existing_cover() {
        let mut book = Book::new();
        book.metadata.cover_image_id = Some("already-set".into());
        book.add_resource(
            "cover-image",
            "images/cover.jpg",
            vec![0xFF, 0xD8],
            "image/jpeg",
        );

        let handler = CoverHandler;
        let result = handler.apply(book).unwrap();

        assert_eq!(
            result.metadata.cover_image_id.as_deref(),
            Some("already-set")
        );
    }

    #[test]
    fn falls_back_to_first_image() {
        let mut book = Book::new();
        book.add_resource("img1", "images/photo.jpg", vec![0xFF, 0xD8], "image/jpeg");

        let handler = CoverHandler;
        let result = handler.apply(book).unwrap();

        assert_eq!(result.metadata.cover_image_id.as_deref(), Some("img1"));
    }

    #[test]
    fn no_cover_when_no_images() {
        let mut book = Book::new();
        book.add_chapter(Chapter {
            title: None,
            content: "<p>text only</p>".into(),
            id: Some("ch1".into()),
        });

        let handler = CoverHandler;
        let result = handler.apply(book).unwrap();

        assert!(result.metadata.cover_image_id.is_none());
    }
}
