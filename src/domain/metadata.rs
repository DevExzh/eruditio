use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// Metadata associated with an ebook. Covers Dublin Core fields,
/// common extensions (series, sorting), and a catch-all map.
#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub title: Option<String>,
    pub title_sort: Option<String>,
    pub authors: Vec<String>,
    pub author_sort: Option<String>,
    pub publisher: Option<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
    pub isbn: Option<String>,
    pub publication_date: Option<DateTime<Utc>>,
    pub description: Option<String>,
    pub subjects: Vec<String>,
    pub series: Option<String>,
    pub series_index: Option<f64>,
    pub rights: Option<String>,
    /// Reference to a manifest item ID containing the cover image.
    pub cover_image_id: Option<String>,
    /// All `dc:date` elements from the source, stored as `(opf:event, raw_value)` pairs
    /// for roundtrip preservation. The first tuple element is the `opf:event` attribute
    /// (e.g. `"publication"`, `"conversion"`), or `None` if the attribute was absent.
    pub additional_dates: Vec<(Option<String>, String)>,
    /// Catch-all for format-specific metadata that doesn't map to a named field.
    pub extended: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_metadata_has_no_title() {
        let m = Metadata::default();
        assert!(m.title.is_none());
        assert!(m.authors.is_empty());
    }

    #[test]
    fn metadata_clone_is_independent() {
        let original = Metadata {
            title: Some("Original".into()),
            ..Default::default()
        };
        let mut cloned = original.clone();
        cloned.title = Some("Cloned".into());
        assert_eq!(original.title.as_deref(), Some("Original"));
        assert_eq!(cloned.title.as_deref(), Some("Cloned"));
    }
}
