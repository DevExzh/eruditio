use crate::domain::{Book, TocItem};
use crate::error::{EruditioError, Result};
use crate::formats::common::html_utils::escape_html;
use std::fmt::Write as FmtWrite;
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

fn generate_container_xml() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#
}

/// Generates the OPF package document XML from a `Book`.
fn generate_opf(book: &Book) -> String {
    let mut xml = String::with_capacity(4096);

    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    xml.push('\n');

    // Use the preserved OPF version from source, defaulting to "3.0".
    let opf_version = book
        .metadata
        .extended
        .get("opf:version")
        .map(|s| s.as_str())
        .unwrap_or("3.0");
    xml.push_str(&format!(
        r#"<package xmlns="http://www.idpf.org/2007/opf" version="{}" unique-identifier="uid">"#,
        opf_version
    ));
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
    xml.push_str(r#"  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">"#);
    xml.push('\n');

    if let Some(ref title) = m.title {
        xml.push_str("    <dc:title>");
        xml.push_str(&escape_html(title));
        xml.push_str("</dc:title>\n");
    }
    for (i, author) in m.authors.iter().enumerate() {
        if i == 0 {
            if let Some(ref sort) = m.author_sort {
                xml.push_str("    <dc:creator opf:file-as=\"");
                xml.push_str(&escape_html(sort));
                xml.push_str("\">");
            } else {
                xml.push_str("    <dc:creator>");
            }
        } else {
            xml.push_str("    <dc:creator>");
        }
        xml.push_str(&escape_html(author));
        xml.push_str("</dc:creator>\n");
    }
    if let Some(ref lang) = m.language {
        xml.push_str("    <dc:language>");
        xml.push_str(&escape_html(lang));
        xml.push_str("</dc:language>\n");
    } else {
        xml.push_str("    <dc:language>en</dc:language>\n");
    }
    if let Some(ref publisher) = m.publisher {
        xml.push_str("    <dc:publisher>");
        xml.push_str(&escape_html(publisher));
        xml.push_str("</dc:publisher>\n");
    }
    if let Some(ref identifier) = m.identifier {
        xml.push_str("    <dc:identifier id=\"uid\">");
        xml.push_str(&escape_html(identifier));
        xml.push_str("</dc:identifier>\n");
    } else {
        xml.push_str("    <dc:identifier id=\"uid\">urn:uuid:00000000-0000-0000-0000-000000000000</dc:identifier>\n");
    }
    if let Some(ref isbn) = m.isbn {
        xml.push_str("    <dc:identifier opf:scheme=\"ISBN\">");
        xml.push_str(&escape_html(isbn));
        xml.push_str("</dc:identifier>\n");
    }
    if let Some(ref desc) = m.description {
        xml.push_str("    <dc:description>");
        xml.push_str(&escape_html(desc));
        xml.push_str("</dc:description>\n");
    }
    for subject in &m.subjects {
        xml.push_str("    <dc:subject>");
        xml.push_str(&escape_html(subject));
        xml.push_str("</dc:subject>\n");
    }
    if let Some(ref rights) = m.rights {
        xml.push_str("    <dc:rights>");
        xml.push_str(&escape_html(rights));
        xml.push_str("</dc:rights>\n");
    }
    // Write dc:date elements: prefer roundtrip-preserved entries, fall back
    // to the parsed publication_date.
    if !m.additional_dates.is_empty() {
        for (event, value) in &m.additional_dates {
            if let Some(ev) = event {
                xml.push_str("    <dc:date opf:event=\"");
                xml.push_str(&escape_html(ev));
                xml.push_str("\">");
            } else {
                xml.push_str("    <dc:date>");
            }
            xml.push_str(&escape_html(value));
            xml.push_str("</dc:date>\n");
        }
    } else if let Some(ref date) = m.publication_date {
        xml.push_str(&format!(
            "    <dc:date>{}</dc:date>\n",
            date.format("%Y-%m-%d")
        ));
    }
    if let Some(ref cover_id) = m.cover_image_id {
        xml.push_str("    <meta name=\"cover\" content=\"");
        xml.push_str(&escape_html(cover_id));
        xml.push_str("\"/>\n");
    }
    if let Some(ref series) = m.series {
        xml.push_str("    <meta name=\"calibre:series\" content=\"");
        xml.push_str(&escape_html(series));
        xml.push_str("\"/>\n");
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
        xml.push_str("    <item id=\"");
        xml.push_str(&escape_html(&item.id));
        xml.push_str("\" href=\"");
        xml.push_str(&escape_html(&item.href));
        xml.push_str("\" media-type=\"");
        xml.push_str(&escape_html(&item.media_type));
        xml.push('"');
        if !item.properties.is_empty() {
            xml.push_str(" properties=\"");
            xml.push_str(&escape_html(&item.properties.join(" ")));
            xml.push('"');
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
        xml.push_str("    <itemref idref=\"");
        xml.push_str(&escape_html(&spine_item.manifest_id));
        xml.push('"');
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
        xml.push_str("    <reference type=\"");
        xml.push_str(&escape_html(r.ref_type.as_str()));
        xml.push_str("\" title=\"");
        xml.push_str(&escape_html(&r.title));
        xml.push_str("\" href=\"");
        xml.push_str(&escape_html(&r.href));
        xml.push_str("\"/>\n");
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
    xml.push_str("    <meta name=\"dtb:uid\" content=\"");
    xml.push_str(&escape_html(uid));
    xml.push_str("\"/>\n");
    xml.push_str("    <meta name=\"dtb:depth\" content=\"1\"/>\n");
    xml.push_str("    <meta name=\"dtb:totalPageCount\" content=\"0\"/>\n");
    xml.push_str("    <meta name=\"dtb:maxPageNumber\" content=\"0\"/>\n");
    xml.push_str("  </head>\n");
    xml.push_str("  <docTitle><text>");
    xml.push_str(&escape_html(title));
    xml.push_str("</text></docTitle>\n");
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
    write_ncx_navpoint_depth(item, xml, play_order, indent, 0);
}

/// Maximum nesting depth for NCX nav-points, matching `MAX_TOC_DEPTH` in `domain::toc`.
const MAX_NCX_DEPTH: usize = 64;

fn write_ncx_navpoint_depth(
    item: &TocItem,
    xml: &mut String,
    play_order: &mut u32,
    indent: usize,
    depth: usize,
) {
    if depth >= MAX_NCX_DEPTH {
        return;
    }
    // Use a fixed indentation buffer to avoid "  ".repeat() allocation per call.
    const INDENT_BUF: &str = "                                ";
    let pad_len = (indent * 2).min(INDENT_BUF.len());
    let pad = &INDENT_BUF[..pad_len];

    let id = item
        .id
        .as_deref()
        .map(String::from)
        .unwrap_or_else(|| format!("navpoint-{}", *play_order));

    xml.push_str(pad);
    xml.push_str("<navPoint id=\"");
    xml.push_str(&escape_html(&id));
    xml.push_str("\" playOrder=\"");
    let _ = write!(xml, "{}", *play_order);
    xml.push_str("\">\n");
    *play_order += 1;

    xml.push_str(pad);
    xml.push_str("  <navLabel><text>");
    xml.push_str(&escape_html(&item.title));
    xml.push_str("</text></navLabel>\n");

    xml.push_str(pad);
    xml.push_str("  <content src=\"");
    xml.push_str(&escape_html(&item.href));
    xml.push_str("\"/>\n");

    for child in &item.children {
        write_ncx_navpoint_depth(child, xml, play_order, indent + 1, depth + 1);
    }

    xml.push_str(pad);
    xml.push_str("</navPoint>\n");
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
    fn generates_opf_with_isbn_identifier() {
        let mut book = sample_book();
        book.metadata.isbn = Some("978-3-16-148410-0".into());
        let opf = generate_opf(&book);
        assert!(
            opf.contains(r#"<dc:identifier opf:scheme="ISBN">978-3-16-148410-0</dc:identifier>"#)
        );
        // The primary identifier should still be present
        assert!(opf.contains(r#"<dc:identifier id="uid">urn:test:12345</dc:identifier>"#));
    }

    #[test]
    fn generates_opf_without_isbn_when_absent() {
        let book = sample_book();
        let opf = generate_opf(&book);
        assert!(!opf.contains("opf:scheme=\"ISBN\""));
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

    #[test]
    fn css_resources_included_in_epub_zip() {
        use crate::domain::ManifestItem;
        use std::io::Read as _;

        let mut book = sample_book();

        // Add CSS as binary data (via add_resource, the public API).
        book.add_resource(
            "stylesheet",
            "styles/stylesheet.css",
            b"body { margin: 0; }".to_vec(),
            "text/css",
        );

        // Also add CSS as ManifestData::Text (the way the EPUB reader loads it).
        let css_item = ManifestItem::new("extra-css", "styles/extra.css", "text/css")
            .with_text("h1 { color: red; }");
        book.manifest.insert(css_item);

        let mut output = Cursor::new(Vec::new());
        write_epub(&book, &mut output).unwrap();

        output.set_position(0);
        let mut archive = zip::ZipArchive::new(output).unwrap();

        // Verify both CSS files exist in the ZIP at the correct paths.
        {
            let mut css_file = archive
                .by_name("OEBPS/styles/stylesheet.css")
                .expect("CSS file (Inline) missing from EPUB ZIP");
            let mut contents = Vec::new();
            css_file.read_to_end(&mut contents).unwrap();
            assert_eq!(contents, b"body { margin: 0; }");
        }
        {
            let mut css_file = archive
                .by_name("OEBPS/styles/extra.css")
                .expect("CSS file (Text) missing from EPUB ZIP");
            let mut contents = String::new();
            css_file.read_to_string(&mut contents).unwrap();
            assert_eq!(contents, "h1 { color: red; }");
        }
    }

    #[test]
    fn css_items_appear_in_opf_manifest() {
        use crate::domain::ManifestItem;

        let mut book = sample_book();

        book.add_resource(
            "stylesheet",
            "styles/stylesheet.css",
            b"body { margin: 0; }".to_vec(),
            "text/css",
        );

        let css_item = ManifestItem::new("extra-css", "styles/extra.css", "text/css")
            .with_text("h1 { color: red; }");
        book.manifest.insert(css_item);

        let opf = generate_opf(&book);

        // Both CSS items must be listed in the OPF <manifest> with correct media-type.
        assert!(
            opf.contains(r#"id="stylesheet" href="styles/stylesheet.css" media-type="text/css""#),
            "OPF manifest missing stylesheet item"
        );
        assert!(
            opf.contains(r#"id="extra-css" href="styles/extra.css" media-type="text/css""#),
            "OPF manifest missing extra-css item"
        );
    }

    #[test]
    fn ncx_respects_depth_limit() {
        // Build a deeply nested TOC (deeper than MAX_NCX_DEPTH) and ensure
        // the writer terminates without panicking or infinite recursion.
        let mut item = TocItem {
            title: "Leaf".into(),
            href: "leaf.xhtml".into(),
            id: Some("leaf".into()),
            children: Vec::new(),
            play_order: None,
        };
        // Nest 100 levels deep (well beyond MAX_NCX_DEPTH = 64).
        for i in (0..100).rev() {
            item = TocItem {
                title: format!("Level {}", i),
                href: format!("ch{}.xhtml", i),
                id: Some(format!("nav-{}", i)),
                children: vec![item],
                play_order: None,
            };
        }

        let mut xml = String::new();
        let mut play_order = 1u32;
        write_ncx_navpoint(&item, &mut xml, &mut play_order, 0);

        // Should contain the top-level navPoint but stop well before level 100.
        assert!(xml.contains("Level 0"));
        // play_order should not reach 100, meaning recursion was cut off.
        assert!(
            (play_order as usize) <= MAX_NCX_DEPTH + 1,
            "play_order {} exceeds depth limit",
            play_order
        );
    }

    #[test]
    fn generates_opf_with_author_sort_file_as() {
        let mut book = sample_book();
        book.metadata.author_sort = Some("Author, Test".into());
        let opf = generate_opf(&book);
        assert!(
            opf.contains(r#"<dc:creator opf:file-as="Author, Test">Test Author</dc:creator>"#),
            "First author should have opf:file-as attribute. Got:\n{}",
            opf
        );
    }

    #[test]
    fn generates_opf_without_file_as_when_author_sort_absent() {
        let book = sample_book();
        assert!(book.metadata.author_sort.is_none());
        let opf = generate_opf(&book);
        assert!(
            opf.contains("<dc:creator>Test Author</dc:creator>"),
            "Creator should have no opf:file-as when author_sort is None. Got:\n{}",
            opf
        );
        assert!(
            !opf.contains("opf:file-as"),
            "opf:file-as should not appear when author_sort is None. Got:\n{}",
            opf
        );
    }

    #[test]
    fn generates_opf_file_as_only_on_first_author() {
        let mut book = sample_book();
        book.metadata.authors.push("Second Author".into());
        book.metadata.author_sort = Some("Author, Test".into());
        let opf = generate_opf(&book);
        assert!(
            opf.contains(r#"<dc:creator opf:file-as="Author, Test">Test Author</dc:creator>"#),
            "First author should have opf:file-as. Got:\n{}",
            opf
        );
        assert!(
            opf.contains("<dc:creator>Second Author</dc:creator>"),
            "Second author should not have opf:file-as. Got:\n{}",
            opf
        );
    }

    #[test]
    fn author_sort_round_trips_through_opf() {
        use crate::formats::epub::opf::parse_opf_xml;

        let mut book = sample_book();
        book.metadata.author_sort = Some("Author, Test".into());

        // Generate OPF XML from the book
        let opf_xml = generate_opf(&book);

        // Parse the generated OPF XML back
        let data = parse_opf_xml(&opf_xml).unwrap();

        assert_eq!(data.metadata.authors, vec!["Test Author"]);
        assert_eq!(
            data.metadata.author_sort.as_deref(),
            Some("Author, Test"),
            "author_sort should survive OPF round-trip"
        );
    }

    #[test]
    fn opf_version_defaults_to_3() {
        let book = sample_book();
        let opf = generate_opf(&book);
        assert!(
            opf.contains(r#"version="3.0""#),
            "Default OPF version should be 3.0 when no source version is set. Got:\n{}",
            opf
        );
    }

    #[test]
    fn opf_version_preserves_2_0() {
        let mut book = sample_book();
        book.metadata
            .extended
            .insert("opf:version".into(), "2.0".into());
        let opf = generate_opf(&book);
        assert!(
            opf.contains(r#"version="2.0""#),
            "OPF version should be preserved as 2.0 from source. Got:\n{}",
            opf
        );
        assert!(
            !opf.contains(r#"version="3.0""#),
            "OPF version should NOT be 3.0 when source was 2.0. Got:\n{}",
            opf
        );
    }

    #[test]
    fn opf_version_round_trips_through_opf() {
        use crate::formats::epub::opf::parse_opf_xml;

        let mut book = sample_book();
        book.metadata
            .extended
            .insert("opf:version".into(), "2.0".into());

        let opf_xml = generate_opf(&book);
        let data = parse_opf_xml(&opf_xml).unwrap();

        assert_eq!(
            data.metadata
                .extended
                .get("opf:version")
                .map(|s| s.as_str()),
            Some("2.0"),
            "OPF version 2.0 should survive round-trip"
        );
    }

    #[test]
    fn multiple_dates_round_trip_through_opf() {
        use crate::formats::epub::opf::parse_opf_xml;

        let mut book = sample_book();
        book.metadata.additional_dates = vec![
            (Some("publication".into()), "2008-06-27".into()),
            (
                Some("conversion".into()),
                "2026-03-01T08:32:03.786809+00:00".into(),
            ),
        ];

        let opf_xml = generate_opf(&book);
        assert!(
            opf_xml.contains(r#"opf:event="publication">2008-06-27</dc:date>"#),
            "Publication date should appear in output. Got:\n{}",
            opf_xml
        );
        assert!(
            opf_xml
                .contains(r#"opf:event="conversion">2026-03-01T08:32:03.786809+00:00</dc:date>"#),
            "Conversion date should appear in output. Got:\n{}",
            opf_xml
        );

        // Parse back and verify both dates survived.
        let data = parse_opf_xml(&opf_xml).unwrap();
        assert_eq!(
            data.metadata.additional_dates.len(),
            2,
            "Both dates should survive round-trip"
        );
        assert_eq!(data.metadata.additional_dates[0].1, "2008-06-27");
        assert_eq!(
            data.metadata.additional_dates[1].1,
            "2026-03-01T08:32:03.786809+00:00"
        );
    }

    #[test]
    fn single_date_without_additional_dates_still_emitted() {
        let mut book = sample_book();
        book.metadata.publication_date = Some(
            chrono::NaiveDate::from_ymd_opt(2024, 3, 15)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
        );
        // additional_dates is empty; the writer should fall back to publication_date
        let opf = generate_opf(&book);
        assert!(
            opf.contains("<dc:date>2024-03-15</dc:date>"),
            "publication_date should be emitted when additional_dates is empty. Got:\n{}",
            opf
        );
    }
}
