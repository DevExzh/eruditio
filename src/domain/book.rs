use super::guide::Guide;
use super::manifest::{Manifest, ManifestItem};
use super::metadata::Metadata;
use super::spine::Spine;
use super::toc::TocItem;

/// A convenience struct for building chapters before adding them to a `Book`.
/// Not stored directly — the book uses manifest + spine internally.
#[derive(Debug, Clone)]
pub struct Chapter {
    pub title: Option<String>,
    /// The content of the chapter (typically XHTML).
    pub content: String,
    /// Optional identifier for internal linking.
    pub id: Option<String>,
}

/// A borrowed view of a chapter (avoids cloning content).
#[derive(Debug, Clone)]
pub struct ChapterView<'a> {
    pub title: Option<&'a str>,
    pub content: &'a str,
    pub id: &'a str,
}

/// A convenience view of a resource in the book.
#[derive(Debug, Clone)]
pub struct ResourceView<'a> {
    pub id: &'a str,
    pub href: &'a str,
    pub data: &'a [u8],
    pub media_type: &'a str,
}

/// Represents an electronic book: metadata, content, resources, structure.
///
/// Internally uses an OEB-like model with a manifest (all files), spine (reading order),
/// table of contents (navigation), and guide (semantic landmarks).
#[derive(Debug, Clone, Default)]
#[must_use]
pub struct Book {
    pub metadata: Metadata,
    pub manifest: Manifest,
    pub spine: Spine,
    pub toc: Vec<TocItem>,
    pub guide: Guide,
}

impl Book {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a text content document to the manifest and spine.
    ///
    /// This is the primary way to add chapters: the content is stored as a
    /// `ManifestItem` with XHTML media type, and a corresponding `SpineItem`
    /// is appended to the reading order.
    pub fn add_chapter(&mut self, chapter: Chapter) {
        let id = chapter
            .id
            .unwrap_or_else(|| format!("chapter_{}", self.spine.len()));
        let href = format!("{}.xhtml", &id);

        let item =
            ManifestItem::new(&id, &href, "application/xhtml+xml").with_text(chapter.content);
        self.manifest.insert(item);
        self.spine.add(&id);

        // If the chapter has a title, add a TOC entry.
        if let Some(title) = chapter.title {
            self.toc.push(TocItem::new(&title, &href));
        }
    }

    /// Adds a binary resource (image, font, stylesheet, etc.) to the manifest.
    /// Resources are not added to the spine (they are not part of reading order).
    pub fn add_resource(
        &mut self,
        id: impl Into<String>,
        href: impl Into<String>,
        data: Vec<u8>,
        media_type: impl Into<String>,
    ) {
        let item = ManifestItem::new(id, href, media_type).with_data(data);
        self.manifest.insert(item);
    }

    /// Returns the chapters (content documents in spine order) as `Chapter` structs.
    ///
    /// This reconstructs the old flat-chapters view from manifest + spine.
    /// Title is derived from the TOC if a matching entry exists.
    pub fn chapters(&self) -> Vec<Chapter> {
        self.spine
            .iter()
            .filter_map(|spine_item| {
                let manifest_item = self.manifest.get(&spine_item.manifest_id)?;
                let content = manifest_item.data.as_text()?.to_string();

                // Try to find a title from the TOC.
                let title = self.find_toc_title(&manifest_item.href).map(String::from);

                Some(Chapter {
                    title,
                    content,
                    id: Some(manifest_item.id.clone()),
                })
            })
            .collect()
    }

    /// Returns views of all non-content resources (images, fonts, stylesheets).
    pub fn resources(&self) -> Vec<ResourceView<'_>> {
        self.manifest
            .iter()
            .filter(|item| {
                // Exclude content documents (they're accessed via chapters()).
                !item.media_type.contains("xhtml") && !item.media_type.contains("html")
            })
            .filter_map(|item| {
                // Return binary data from Inline items, or UTF-8 bytes from Text
                // items (e.g. CSS loaded as text by the EPUB reader).
                let data = match &item.data {
                    super::manifest::ManifestData::Inline(b) => Some(&**b),
                    super::manifest::ManifestData::Text(s) => Some(s.as_bytes()),
                    super::manifest::ManifestData::Empty => None,
                }?;
                Some(ResourceView {
                    id: &item.id,
                    href: &item.href,
                    data,
                    media_type: &item.media_type,
                })
            })
            .collect()
    }

    /// Looks up a resource by manifest ID and returns its binary data.
    ///
    /// Returns the raw bytes for both `Inline` and `Text` manifest items,
    /// so callers can access CSS and other text-based resources loaded from EPUB.
    pub fn resource_data(&self, id: &str) -> Option<&[u8]> {
        self.manifest.get(id).and_then(|item| match &item.data {
            super::manifest::ManifestData::Inline(b) => Some(&**b),
            super::manifest::ManifestData::Text(s) => Some(s.as_bytes()),
            super::manifest::ManifestData::Empty => None,
        })
    }

    /// Returns borrowed views of chapters without cloning content.
    pub fn chapter_views(&self) -> Vec<ChapterView<'_>> {
        self.spine
            .iter()
            .filter_map(|spine_item| {
                let manifest_item = self.manifest.get(&spine_item.manifest_id)?;
                let content = manifest_item.data.as_text()?;
                let title = self.find_toc_title(&manifest_item.href);
                Some(ChapterView {
                    title,
                    content,
                    id: &manifest_item.id,
                })
            })
            .collect()
    }

    /// Returns the number of content chapters in reading order.
    pub fn chapter_count(&self) -> usize {
        self.spine
            .iter()
            .filter(|spine_item| {
                self.manifest
                    .get(&spine_item.manifest_id)
                    .and_then(|item| item.data.as_text())
                    .is_some()
            })
            .count()
    }

    /// Searches the TOC tree for an entry whose href matches (prefix match).
    fn find_toc_title(&self, href: &str) -> Option<&str> {
        fn search<'a>(items: &'a [TocItem], href: &str) -> Option<&'a str> {
            for item in items {
                if item.href == href || href.starts_with(&item.href) {
                    return Some(&item.title);
                }
                if let Some(title) = search(&item.children, href) {
                    return Some(title);
                }
            }
            None
        }
        search(&self.toc, href)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_chapter_populates_manifest_and_spine() {
        let mut book = Book::new();
        book.add_chapter(Chapter {
            title: Some("Intro".into()),
            content: "<p>Hello world</p>".into(),
            id: Some("intro".into()),
        });

        assert_eq!(book.spine.len(), 1);
        assert_eq!(book.manifest.len(), 1);
        assert!(book.manifest.get("intro").is_some());
    }

    #[test]
    fn chapters_round_trip() {
        let mut book = Book::new();
        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Content one</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: None,
            content: "<p>Content two</p>".into(),
            id: Some("ch2".into()),
        });

        let chapters = book.chapters();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].title.as_deref(), Some("Chapter 1"));
        assert!(chapters[0].content.contains("Content one"));
        assert_eq!(chapters[1].id.as_deref(), Some("ch2"));
    }

    #[test]
    fn add_resource_stores_in_manifest() {
        let mut book = Book::new();
        book.add_resource("cover", "images/cover.jpg", vec![0xFF, 0xD8], "image/jpeg");

        assert_eq!(book.manifest.len(), 1);
        let data = book.resource_data("cover").unwrap();
        assert_eq!(data, &[0xFF, 0xD8]);
    }

    #[test]
    fn resources_excludes_content_documents() {
        let mut book = Book::new();
        book.add_chapter(Chapter {
            title: Some("Ch".into()),
            content: "<p>text</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_resource("img", "img.png", vec![0x89, 0x50], "image/png");

        let resources = book.resources();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].id, "img");
    }

    #[test]
    fn resources_includes_text_css() {
        // CSS loaded by the EPUB reader is stored as ManifestData::Text.
        // resources() must return it so downstream writers (e.g. HTMLZ) can
        // access the CSS content.
        let mut book = Book::new();
        book.add_chapter(Chapter {
            title: None,
            content: "<p>text</p>".into(),
            id: Some("ch1".into()),
        });

        let css_item = ManifestItem::new("epub-css", "styles/main.css", "text/css")
            .with_text("body { color: blue; }");
        book.manifest.insert(css_item);

        let resources = book.resources();
        let css_res = resources.iter().find(|r| r.media_type == "text/css");
        assert!(
            css_res.is_some(),
            "CSS stored as Text should appear in resources()"
        );
        assert_eq!(
            std::str::from_utf8(css_res.unwrap().data).unwrap(),
            "body { color: blue; }"
        );
    }

    #[test]
    fn resource_data_returns_text_css() {
        let mut book = Book::new();
        let css_item =
            ManifestItem::new("my-css", "style.css", "text/css").with_text("p { margin: 0; }");
        book.manifest.insert(css_item);

        let data = book.resource_data("my-css");
        assert!(
            data.is_some(),
            "resource_data should return bytes for Text items"
        );
        assert_eq!(
            std::str::from_utf8(data.unwrap()).unwrap(),
            "p { margin: 0; }"
        );
    }
}
