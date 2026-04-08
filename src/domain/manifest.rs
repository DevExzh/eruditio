use std::collections::HashMap;
use std::sync::Arc;

/// How a manifest item's data is stored in memory.
#[derive(Debug, Clone)]
pub enum ManifestData {
    /// Raw binary data (images, fonts, etc.).
    ///
    /// Wrapped in `Arc` so that cloning a `Book` (e.g. in the Kepub writer)
    /// is a cheap reference-count bump instead of a deep copy of every image.
    Inline(Arc<Vec<u8>>),
    /// Text content (XHTML, CSS, NCX, etc.).
    Text(String),
    /// Placeholder — data has not been loaded yet.
    Empty,
}

impl ManifestData {
    /// Returns the text content if this is a `Text` variant.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ManifestData::Text(s) => Some(s),
            _ => None,
        }
    }

    /// Returns the binary data if this is an `Inline` variant.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            ManifestData::Inline(b) => Some(b),
            _ => None,
        }
    }

    /// Returns `true` if the data has not been loaded.
    pub fn is_empty(&self) -> bool {
        matches!(self, ManifestData::Empty)
    }
}

/// A single item in the book's manifest (content document, image, stylesheet, etc.).
#[derive(Debug, Clone)]
pub struct ManifestItem {
    pub id: String,
    pub href: String,
    /// MIME type as a string (e.g. "application/xhtml+xml", "image/jpeg").
    pub media_type: String,
    pub data: ManifestData,
    /// Fallback item ID for unsupported media types.
    pub fallback: Option<String>,
    /// EPUB3 properties (e.g. "nav", "cover-image", "scripted").
    pub properties: Vec<String>,
}

impl ManifestItem {
    pub fn new(
        id: impl Into<String>,
        href: impl Into<String>,
        media_type: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            href: href.into(),
            media_type: media_type.into(),
            data: ManifestData::Empty,
            fallback: None,
            properties: Vec::new(),
        }
    }

    pub fn with_text(mut self, content: impl Into<String>) -> Self {
        self.data = ManifestData::Text(content.into());
        self
    }

    pub fn with_data(mut self, data: Vec<u8>) -> Self {
        self.data = ManifestData::Inline(Arc::new(data));
        self
    }

    pub fn with_properties(mut self, props: Vec<String>) -> Self {
        self.properties = props;
        self
    }

    /// Returns `true` if this item has the given EPUB3 property.
    pub fn has_property(&self, prop: &str) -> bool {
        self.properties.iter().any(|p| p == prop)
    }
}

/// The manifest: a collection of all resources in the book, indexed by ID and href.
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    items: HashMap<String, ManifestItem>,
    href_to_id: HashMap<String, String>,
}

impl Manifest {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts an item into the manifest. Overwrites any existing item with the same ID.
    pub fn insert(&mut self, item: ManifestItem) {
        self.href_to_id.insert(item.href.clone(), item.id.clone());
        self.items.insert(item.id.clone(), item);
    }

    /// Looks up an item by its manifest ID.
    pub fn get(&self, id: &str) -> Option<&ManifestItem> {
        self.items.get(id)
    }

    /// Looks up a mutable item by its manifest ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut ManifestItem> {
        self.items.get_mut(id)
    }

    /// Looks up an item by its href path.
    pub fn get_by_href(&self, href: &str) -> Option<&ManifestItem> {
        self.href_to_id.get(href).and_then(|id| self.items.get(id))
    }

    /// Removes an item by ID, returning it if it existed.
    pub fn remove(&mut self, id: &str) -> Option<ManifestItem> {
        if let Some(item) = self.items.remove(id) {
            self.href_to_id.remove(&item.href);
            Some(item)
        } else {
            None
        }
    }

    /// Returns an iterator over all manifest items.
    pub fn iter(&self) -> impl Iterator<Item = &ManifestItem> {
        self.items.values()
    }

    /// Returns a mutable iterator over all manifest items.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut ManifestItem> {
        self.items.values_mut()
    }

    /// Returns the number of items in the manifest.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns `true` if the manifest contains no items.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Returns all item IDs.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.items.keys().map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_retrieve_by_id() {
        let mut manifest = Manifest::new();
        let item = ManifestItem::new("ch1", "chapter1.xhtml", "application/xhtml+xml")
            .with_text("<html><body>Hello</body></html>");
        manifest.insert(item);

        let retrieved = manifest.get("ch1").unwrap();
        assert_eq!(retrieved.href, "chapter1.xhtml");
        assert!(retrieved.data.as_text().unwrap().contains("Hello"));
    }

    #[test]
    fn retrieve_by_href() {
        let mut manifest = Manifest::new();
        manifest.insert(
            ManifestItem::new("img1", "images/cover.jpg", "image/jpeg").with_data(vec![0xFF, 0xD8]),
        );

        let item = manifest.get_by_href("images/cover.jpg").unwrap();
        assert_eq!(item.id, "img1");
        assert_eq!(item.data.as_bytes().unwrap(), &[0xFF, 0xD8]);
    }

    #[test]
    fn remove_cleans_both_indexes() {
        let mut manifest = Manifest::new();
        manifest.insert(ManifestItem::new("x", "x.html", "text/html"));
        assert_eq!(manifest.len(), 1);

        manifest.remove("x");
        assert!(manifest.get("x").is_none());
        assert!(manifest.get_by_href("x.html").is_none());
        assert!(manifest.is_empty());
    }
}
