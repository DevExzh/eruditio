//! Merges user-provided metadata overrides into a book.

use crate::domain::Book;
use crate::domain::metadata::Metadata;
use crate::domain::traits::Transform;
use crate::error::Result;

/// Merges metadata overrides into a book.
///
/// Non-`None` fields in the override metadata replace the corresponding fields
/// in the book. Empty vecs and `None` fields are left unchanged.
pub struct MetadataMerger {
    overrides: Metadata,
}

impl MetadataMerger {
    pub fn new(overrides: Metadata) -> Self {
        Self { overrides }
    }
}

impl Transform for MetadataMerger {
    fn name(&self) -> &str {
        "metadata_merger"
    }

    fn apply(&self, book: Book) -> Result<Book> {
        let mut result = book;
        let meta = &mut result.metadata;
        let ovr = &self.overrides;

        if ovr.title.is_some() {
            meta.title = ovr.title.clone();
        }
        if ovr.title_sort.is_some() {
            meta.title_sort = ovr.title_sort.clone();
        }
        if !ovr.authors.is_empty() {
            meta.authors = ovr.authors.clone();
        }
        if ovr.author_sort.is_some() {
            meta.author_sort = ovr.author_sort.clone();
        }
        if ovr.publisher.is_some() {
            meta.publisher = ovr.publisher.clone();
        }
        if ovr.language.is_some() {
            meta.language = ovr.language.clone();
        }
        if ovr.identifier.is_some() {
            meta.identifier = ovr.identifier.clone();
        }
        if ovr.isbn.is_some() {
            meta.isbn = ovr.isbn.clone();
        }
        if ovr.publication_date.is_some() {
            meta.publication_date = ovr.publication_date;
        }
        if ovr.description.is_some() {
            meta.description = ovr.description.clone();
        }
        if !ovr.subjects.is_empty() {
            meta.subjects = ovr.subjects.clone();
        }
        if ovr.series.is_some() {
            meta.series = ovr.series.clone();
        }
        if ovr.series_index.is_some() {
            meta.series_index = ovr.series_index;
        }
        if ovr.rights.is_some() {
            meta.rights = ovr.rights.clone();
        }
        if ovr.cover_image_id.is_some() {
            meta.cover_image_id = ovr.cover_image_id.clone();
        }
        // Merge extended: override keys replace, original keys not in overrides are preserved.
        for (key, value) in &ovr.extended {
            meta.extended.insert(key.clone(), value.clone());
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;

    #[test]
    fn overrides_title() {
        let mut book = Book::new();
        book.metadata.title = Some("Original".into());
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut overrides = Metadata::default();
        overrides.title = Some("New Title".into());

        let merger = MetadataMerger::new(overrides);
        let result = merger.apply(book).unwrap();

        assert_eq!(result.metadata.title.as_deref(), Some("New Title"));
    }

    #[test]
    fn preserves_fields_not_overridden() {
        let mut book = Book::new();
        book.metadata.title = Some("Keep Me".into());
        book.metadata.language = Some("en".into());

        let mut overrides = Metadata::default();
        overrides.publisher = Some("New Publisher".into());

        let merger = MetadataMerger::new(overrides);
        let result = merger.apply(book).unwrap();

        assert_eq!(result.metadata.title.as_deref(), Some("Keep Me"));
        assert_eq!(result.metadata.language.as_deref(), Some("en"));
        assert_eq!(result.metadata.publisher.as_deref(), Some("New Publisher"));
    }

    #[test]
    fn overrides_authors_when_non_empty() {
        let mut book = Book::new();
        book.metadata.authors = vec!["Original Author".into()];

        let mut overrides = Metadata::default();
        overrides.authors = vec!["New Author".into()];

        let merger = MetadataMerger::new(overrides);
        let result = merger.apply(book).unwrap();

        assert_eq!(result.metadata.authors, vec!["New Author"]);
    }

    #[test]
    fn empty_authors_override_keeps_original() {
        let mut book = Book::new();
        book.metadata.authors = vec!["Keep".into()];

        let overrides = Metadata::default(); // authors is empty vec

        let merger = MetadataMerger::new(overrides);
        let result = merger.apply(book).unwrap();

        assert_eq!(result.metadata.authors, vec!["Keep"]);
    }

    #[test]
    fn merges_extended_metadata() {
        let mut book = Book::new();
        book.metadata
            .extended
            .insert("existing".into(), "value".into());

        let mut overrides = Metadata::default();
        overrides
            .extended
            .insert("new_key".into(), "new_value".into());

        let merger = MetadataMerger::new(overrides);
        let result = merger.apply(book).unwrap();

        assert_eq!(result.metadata.extended.get("existing").unwrap(), "value");
        assert_eq!(
            result.metadata.extended.get("new_key").unwrap(),
            "new_value"
        );
    }

    #[test]
    fn does_not_mutate_original() {
        let mut book = Book::new();
        book.metadata.title = Some("Original".into());

        let mut overrides = Metadata::default();
        overrides.title = Some("Changed".into());

        let merger = MetadataMerger::new(overrides);
        let _result = merger.apply(book.clone()).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Original"));
    }
}
