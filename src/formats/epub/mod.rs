use crate::domain::{Book, FormatReader, FormatWriter, TocItem};
use crate::error::Result;
use std::io::{Read, Write};
use zip::ZipArchive;

pub mod container;
pub mod content;
pub mod mimetype;
pub mod nav;
pub mod ncx;
pub mod opf;
pub mod writer;

/// EPUB format reader.
#[derive(Default)]
pub struct EpubReader;

impl EpubReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for EpubReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let buffer = crate::formats::common::read_capped(reader)?;
        let cursor = std::io::Cursor::new(buffer);

        let mut archive = ZipArchive::new(cursor)?;

        // 1. Verify mimetype
        mimetype::verify_mimetype(&mut archive)?;

        // 2. Find OPF path from container.xml
        let opf_path = container::find_opf_path(&mut archive)?;

        // 3. Parse the full OPF (metadata, manifest, spine, guide)
        let opf_data = opf::parse_opf(&mut archive, &opf_path)?;

        // 4. Load all manifest item data from the ZIP (parallel decompression)
        let opf_dir = content::opf_directory(&opf_path);
        let mut manifest = opf_data.manifest;
        content::load_manifest_data_parallel(archive, &mut manifest, &opf_dir)?;

        // 5. Parse TOC (prefer EPUB3 nav, fall back to NCX)
        let toc = parse_toc(&manifest, &opf_data.ncx_id);

        // 6. Build Book
        let book = Book {
            metadata: opf_data.metadata,
            manifest,
            spine: opf_data.spine,
            toc,
            guide: opf_data.guide,
        };

        Ok(book)
    }
}

/// Extracts the table of contents from the manifest.
///
/// Prefers the EPUB3 nav document (item with `"nav"` property) over the
/// EPUB2 NCX (referenced by `ncx_id`). Returns an empty Vec if neither
/// is available or parseable.
fn parse_toc(manifest: &crate::domain::Manifest, ncx_id: &Option<String>) -> Vec<TocItem> {
    // Try EPUB3 nav document first.
    for item in manifest.iter() {
        if item.has_property("nav")
            && let Some(xhtml) = item.data.as_text()
            && let Ok(toc) = nav::parse_nav(xhtml)
            && !toc.is_empty()
        {
            return toc;
        }
    }

    // Fall back to NCX.
    if let Some(id) = ncx_id
        && let Some(item) = manifest.get(id)
        && let Some(xml) = item.data.as_text()
        && let Ok(toc) = ncx::parse_ncx(xml)
    {
        return toc;
    }

    Vec::new()
}

/// EPUB format writer.
#[derive(Default)]
pub struct EpubWriter;

impl EpubWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for EpubWriter {
    fn write_book(&self, book: &Book, output: &mut dyn Write) -> Result<()> {
        // ZIP creation requires Seek, so buffer into a Cursor first.
        let mut cursor = std::io::Cursor::new(Vec::new());
        writer::write_epub(book, &mut cursor)?;
        output.write_all(cursor.get_ref())?;
        Ok(())
    }
}
