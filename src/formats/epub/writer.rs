use crate::domain::{Book, TocItem};
use crate::error::{EruditioError, Result};
use crate::formats::common::html_utils::escape_html;
use std::io::{Seek, Write};
use zip::CompressionMethod;
use zip::ZipWriter;
use zip::write::FileOptions;

/// Writes a `Book` as a valid EPUB archive to the given writer.
pub(crate) fn write_epub<W: Write + Seek>(book: &Book, writer: W) -> Result<()> {
    let mut zip = ZipWriter::new(writer);
    let stored: FileOptions<'_, ()> =
        FileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated: FileOptions<'_, ()> =
        FileOptions::default().compression_method(CompressionMethod::Deflated);

    // 1. mimetype — must be first, uncompressed.
    zip.start_file("mimetype", stored)
        .map_err(|e| EruditioError::Format(format!("Failed to write mimetype: {}", e)))?;
    zip.write_all(b"application/epub+zip")?;

    // 2. META-INF/container.xml
    zip.start_file("META-INF/container.xml", deflated)
        .map_err(|e| EruditioError::Format(format!("Failed to write container.xml: {}", e)))?;
    zip.write_all(generate_container_xml().as_bytes())?;

    // 3. OPF
    let opf_path = "OEBPS/content.opf";
    let opf_xml = generate_opf(book);
    zip.start_file(opf_path, deflated)
        .map_err(|e| EruditioError::Format(format!("Failed to write OPF: {}", e)))?;
    zip.write_all(opf_xml.as_bytes())?;

    // 4. NCX (for EPUB2 compatibility)
    let ncx_xml = generate_ncx(book);
    zip.start_file("OEBPS/toc.ncx", deflated)
        .map_err(|e| EruditioError::Format(format!("Failed to write NCX: {}", e)))?;
    zip.write_all(ncx_xml.as_bytes())?;

    // 5. Write all manifest items (content + resources).
    // Skip structural files that are already written above.
    const STRUCTURAL_HREFS: &[&str] = &["toc.ncx", "content.opf"];
    for item in book.manifest.iter() {
        if STRUCTURAL_HREFS.contains(&item.href.as_str()) {
            continue;
        }
        let zip_path = format!("OEBPS/{}", &item.href);
        zip.start_file(&zip_path, deflated)
            .map_err(|e| EruditioError::Format(format!("Failed to write {}: {}", zip_path, e)))?;
        match &item.data {
            crate::domain::ManifestData::Text(text) => {
                zip.write_all(text.as_bytes())?;
            },
            crate::domain::ManifestData::Inline(bytes) => {
                zip.write_all(bytes)?;
            },
            crate::domain::ManifestData::Empty => {},
        }
    }

    zip.finish()
        .map_err(|e| EruditioError::Format(format!("Failed to finalize EPUB: {}", e)))?;

    Ok(())
}

fn generate_container_xml() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#
        .to_string()
}

/// Generates the OPF package document XML from a `Book`.
fn generate_opf(book: &Book) -> String {
    let mut xml = String::with_capacity(4096);

    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    xml.push('\n');
    xml.push_str(
        r#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">"#,
    );
    xml.push('\n');

    // Metadata
    generate_opf_metadata(book, &mut xml);

    // Manifest
    generate_opf_manifest(book, &mut xml);

    // Spine
    generate_opf_spine(book, &mut xml);

    // Guide
    generate_opf_guide(book, &mut xml);

    xml.push_str("</package>\n");
    xml
}

fn generate_opf_metadata(book: &Book, xml: &mut String) {
    let m = &book.metadata;
    xml.push_str(r#"  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">"#);
    xml.push('\n');

    if let Some(ref title) = m.title {
        xml.push_str(&format!(
            "    <dc:title>{}</dc:title>\n",
            escape_html(title)
        ));
    }
    for author in &m.authors {
        xml.push_str(&format!(
            "    <dc:creator>{}</dc:creator>\n",
            escape_html(author)
        ));
    }
    if let Some(ref lang) = m.language {
        xml.push_str(&format!(
            "    <dc:language>{}</dc:language>\n",
            escape_html(lang)
        ));
    } else {
        xml.push_str("    <dc:language>en</dc:language>\n");
    }
    if let Some(ref publisher) = m.publisher {
        xml.push_str(&format!(
            "    <dc:publisher>{}</dc:publisher>\n",
            escape_html(publisher)
        ));
    }
    if let Some(ref identifier) = m.identifier {
        xml.push_str(&format!(
            "    <dc:identifier id=\"uid\">{}</dc:identifier>\n",
            escape_html(identifier)
        ));
    } else {
        xml.push_str("    <dc:identifier id=\"uid\">urn:uuid:00000000-0000-0000-0000-000000000000</dc:identifier>\n");
    }
    if let Some(ref desc) = m.description {
        xml.push_str(&format!(
            "    <dc:description>{}</dc:description>\n",
            escape_html(desc)
        ));
    }
    for subject in &m.subjects {
        xml.push_str(&format!(
            "    <dc:subject>{}</dc:subject>\n",
            escape_html(subject)
        ));
    }
    if let Some(ref rights) = m.rights {
        xml.push_str(&format!(
            "    <dc:rights>{}</dc:rights>\n",
            escape_html(rights)
        ));
    }
    if let Some(ref date) = m.publication_date {
        xml.push_str(&format!(
            "    <dc:date>{}</dc:date>\n",
            date.format("%Y-%m-%d")
        ));
    }
    if let Some(ref cover_id) = m.cover_image_id {
        xml.push_str(&format!(
            "    <meta name=\"cover\" content=\"{}\"/>\n",
            escape_html(cover_id)
        ));
    }
    if let Some(ref series) = m.series {
        xml.push_str(&format!(
            "    <meta name=\"calibre:series\" content=\"{}\"/>\n",
            escape_html(series)
        ));
    }
    if let Some(idx) = m.series_index {
        xml.push_str(&format!(
            "    <meta name=\"calibre:series_index\" content=\"{}\"/>\n",
            idx
        ));
    }

    xml.push_str("  </metadata>\n");
}

fn generate_opf_manifest(book: &Book, xml: &mut String) {
    xml.push_str("  <manifest>\n");

    // NCX entry (always included for EPUB2 compat).
    xml.push_str(
        "    <item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n",
    );

    // All manifest items (skip NCX — already emitted above).
    for item in book.manifest.iter() {
        if item.href == "toc.ncx" || item.id == "ncx" {
            continue;
        }
        xml.push_str(&format!(
            "    <item id=\"{}\" href=\"{}\" media-type=\"{}\"",
            escape_html(&item.id),
            escape_html(&item.href),
            escape_html(&item.media_type),
        ));
        if !item.properties.is_empty() {
            xml.push_str(&format!(
                " properties=\"{}\"",
                escape_html(&item.properties.join(" "))
            ));
        }
        xml.push_str("/>\n");
    }

    xml.push_str("  </manifest>\n");
}

fn generate_opf_spine(book: &Book, xml: &mut String) {
    xml.push_str("  <spine toc=\"ncx\"");
    if let Some(ppd) = &book.spine.page_progression_direction {
        let dir = match ppd {
            crate::domain::PageProgression::Ltr => "ltr",
            crate::domain::PageProgression::Rtl => "rtl",
        };
        xml.push_str(&format!(" page-progression-direction=\"{}\"", dir));
    }
    xml.push_str(">\n");

    for spine_item in book.spine.iter() {
        xml.push_str(&format!(
            "    <itemref idref=\"{}\"",
            escape_html(&spine_item.manifest_id)
        ));
        if !spine_item.linear {
            xml.push_str(" linear=\"no\"");
        }
        xml.push_str("/>\n");
    }

    xml.push_str("  </spine>\n");
}

fn generate_opf_guide(book: &Book, xml: &mut String) {
    if book.guide.is_empty() {
        return;
    }
    xml.push_str("  <guide>\n");
    for r in &book.guide.references {
        xml.push_str(&format!(
            "    <reference type=\"{}\" title=\"{}\" href=\"{}\"/>\n",
            escape_html(r.ref_type.as_str()),
            escape_html(&r.title),
            escape_html(&r.href),
        ));
    }
    xml.push_str("  </guide>\n");
}

/// Generates an NCX document from the book's TOC.
fn generate_ncx(book: &Book) -> String {
    let uid = book
        .metadata
        .identifier
        .as_deref()
        .unwrap_or("urn:uuid:00000000-0000-0000-0000-000000000000");
    let title = book.metadata.title.as_deref().unwrap_or("Untitled");

    let mut xml = String::with_capacity(2048);
    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    xml.push('\n');
    xml.push_str(r#"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">"#);
    xml.push('\n');
    xml.push_str("  <head>\n");
    xml.push_str(&format!(
        "    <meta name=\"dtb:uid\" content=\"{}\"/>\n",
        escape_html(uid)
    ));
    xml.push_str("    <meta name=\"dtb:depth\" content=\"1\"/>\n");
    xml.push_str("    <meta name=\"dtb:totalPageCount\" content=\"0\"/>\n");
    xml.push_str("    <meta name=\"dtb:maxPageNumber\" content=\"0\"/>\n");
    xml.push_str("  </head>\n");
    xml.push_str(&format!(
        "  <docTitle><text>{}</text></docTitle>\n",
        escape_html(title)
    ));
    xml.push_str("  <navMap>\n");

    let mut play_order = 1u32;
    for item in &book.toc {
        write_ncx_navpoint(item, &mut xml, &mut play_order, 2);
    }

    xml.push_str("  </navMap>\n");
    xml.push_str("</ncx>\n");
    xml
}

fn write_ncx_navpoint(item: &TocItem, xml: &mut String, play_order: &mut u32, indent: usize) {
    let pad = "  ".repeat(indent);
    let id = item
        .id
        .as_deref()
        .map(String::from)
        .unwrap_or_else(|| format!("navpoint-{}", *play_order));

    xml.push_str(&format!(
        "{}<navPoint id=\"{}\" playOrder=\"{}\">\n",
        pad,
        escape_html(&id),
        *play_order,
    ));
    *play_order += 1;

    xml.push_str(&format!(
        "{}  <navLabel><text>{}</text></navLabel>\n",
        pad,
        escape_html(&item.title)
    ));
    xml.push_str(&format!(
        "{}  <content src=\"{}\"/>\n",
        pad,
        escape_html(&item.href)
    ));

    for child in &item.children {
        write_ncx_navpoint(child, xml, play_order, indent + 1);
    }

    xml.push_str(&format!("{}</navPoint>\n", pad));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Book, Chapter, GuideReference, GuideType};
    use std::io::Cursor;

    fn sample_book() -> Book {
        let mut book = Book::new();
        book.metadata.title = Some("Test Book".into());
        book.metadata.authors.push("Test Author".into());
        book.metadata.language = Some("en".into());
        book.metadata.identifier = Some("urn:test:12345".into());

        book.add_chapter(&Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello World</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(&Chapter {
            title: Some("Chapter 2".into()),
            content: "<p>Goodbye World</p>".into(),
            id: Some("ch2".into()),
        });

        book.add_resource(
            "cover",
            "images/cover.jpg",
            vec![0xFF, 0xD8, 0xFF],
            "image/jpeg",
        );

        book.guide.push(GuideReference {
            ref_type: GuideType::Cover,
            title: "Cover".into(),
            href: "ch1.xhtml".into(),
        });

        book
    }

    #[test]
    fn generates_valid_container_xml() {
        let xml = generate_container_xml();
        assert!(xml.contains("OEBPS/content.opf"));
        assert!(xml.contains("application/oebps-package+xml"));
    }

    #[test]
    fn generates_opf_with_metadata() {
        let book = sample_book();
        let opf = generate_opf(&book);
        assert!(opf.contains("<dc:title>Test Book</dc:title>"));
        assert!(opf.contains("<dc:creator>Test Author</dc:creator>"));
        assert!(opf.contains("<dc:language>en</dc:language>"));
    }

    #[test]
    fn generates_opf_manifest_and_spine() {
        let book = sample_book();
        let opf = generate_opf(&book);
        assert!(opf.contains("id=\"ch1\""));
        assert!(opf.contains("id=\"ch2\""));
        assert!(opf.contains("id=\"cover\""));
        assert!(opf.contains("idref=\"ch1\""));
        assert!(opf.contains("idref=\"ch2\""));
        assert!(opf.contains("toc=\"ncx\""));
    }

    #[test]
    fn generates_opf_guide() {
        let book = sample_book();
        let opf = generate_opf(&book);
        assert!(opf.contains("type=\"cover\""));
        assert!(opf.contains("title=\"Cover\""));
    }

    #[test]
    fn generates_ncx_with_toc() {
        let book = sample_book();
        let ncx = generate_ncx(&book);
        assert!(ncx.contains("Chapter 1"));
        assert!(ncx.contains("Chapter 2"));
        assert!(ncx.contains("playOrder=\"1\""));
        assert!(ncx.contains("playOrder=\"2\""));
        assert!(ncx.contains("urn:test:12345"));
    }

    #[test]
    fn write_epub_produces_valid_zip() {
        let book = sample_book();
        let mut output = Cursor::new(Vec::new());
        write_epub(&book, &mut output).unwrap();

        // Verify the ZIP is valid and contains expected files.
        output.set_position(0);
        let mut archive = zip::ZipArchive::new(output).unwrap();

        assert!(archive.by_name("mimetype").is_ok());
        assert!(archive.by_name("META-INF/container.xml").is_ok());
        assert!(archive.by_name("OEBPS/content.opf").is_ok());
        assert!(archive.by_name("OEBPS/toc.ncx").is_ok());
        assert!(archive.by_name("OEBPS/ch1.xhtml").is_ok());
        assert!(archive.by_name("OEBPS/ch2.xhtml").is_ok());
        assert!(archive.by_name("OEBPS/images/cover.jpg").is_ok());
    }

    #[test]
    fn mimetype_is_uncompressed() {
        let book = sample_book();
        let mut output = Cursor::new(Vec::new());
        write_epub(&book, &mut output).unwrap();

        output.set_position(0);
        let mut archive = zip::ZipArchive::new(output).unwrap();
        let mimetype = archive.by_name("mimetype").unwrap();
        assert_eq!(mimetype.compression(), CompressionMethod::Stored);
    }
}
