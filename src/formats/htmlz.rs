//! HTMLZ format: HTML inside a ZIP archive.
//!
//! Produces a calibre-compatible HTMLZ archive with:
//! - `index.html` — HTML content with relative image paths
//! - `metadata.opf` — OPF metadata (Dublin Core)
//! - `images/` — extracted image resources

use crate::domain::{Book, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use crate::formats::common::html_utils::{escape_html, strip_leading_heading};
use crate::formats::common::xml_utils;
use crate::formats::common::MAX_INPUT_SIZE;
use crate::formats::html::HtmlReader;
use quick_xml::events::Event;
use quick_xml::Reader as XmlReader;
use std::io::{Cursor, Read, Seek, Write};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// HTMLZ format reader (HTML inside a ZIP archive).
#[derive(Default)]
pub struct HtmlzReader;

impl HtmlzReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for HtmlzReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut buffer = Vec::new();
        (&mut *reader).take(MAX_INPUT_SIZE).read_to_end(&mut buffer)?;
        let cursor = Cursor::new(buffer);

        let mut archive = ZipArchive::new(cursor)?;

        // Find the first HTML file in the archive.
        let html_name = find_html_file(&mut archive)
            .ok_or_else(|| EruditioError::Format("No HTML file found in HTMLZ archive".into()))?;

        let mut html_file = archive
            .by_name(&html_name)
            .map_err(|_| EruditioError::Format(format!("Failed to read {}", html_name)))?;

        let mut contents = Vec::new();
        html_file.read_to_end(&mut contents)?;
        drop(html_file);

        let mut cursor = Cursor::new(contents);
        let mut book = HtmlReader::new().read_book(&mut cursor)?;

        // Try to read metadata.opf and merge metadata.
        if let Ok(mut opf_file) = archive.by_name("metadata.opf") {
            let mut opf_contents = String::new();
            if opf_file.read_to_string(&mut opf_contents).is_ok() {
                drop(opf_file);
                merge_opf_metadata(&opf_contents, &mut book);
            }
        }

        // Load style.css if present.
        if let Ok(mut css_file) = archive.by_name("style.css") {
            let mut css_data = Vec::new();
            if css_file.read_to_end(&mut css_data).is_ok() && !css_data.is_empty() {
                book.add_resource("style", "style.css", css_data, "text/css");
            }
        }

        // Extract image resources from the archive.
        let file_names: Vec<String> = (0..archive.len())
            .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
            .collect();

        for name in &file_names {
            if name.starts_with("images/") && name.len() > "images/".len() {
                if let Ok(mut file) = archive.by_name(name) {
                    let mut data = Vec::new();
                    if file.read_to_end(&mut data).is_ok() && !data.is_empty() {
                        let filename = name.rsplit('/').next().unwrap_or(name);
                        let media_type = guess_media_type(filename);
                        let id = filename
                            .rsplit_once('.')
                            .map(|(base, _)| base)
                            .unwrap_or(filename);
                        book.add_resource(id, name.as_str(), data, media_type);
                    }
                }
            }
        }

        Ok(book)
    }
}

/// HTMLZ format writer (HTML inside a ZIP archive).
#[derive(Default)]
pub struct HtmlzWriter;

impl HtmlzWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for HtmlzWriter {
    fn write_book(&self, book: &Book, output: &mut dyn Write) -> Result<()> {
        let mut zip_buf = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut zip_buf);
            let options: FileOptions<'_, ()> =
                FileOptions::default().compression_method(CompressionMethod::Deflated);

            // Collect CSS content: use manifest CSS resources, or generate a default
            let resources = book.resources();
            let css_resources: Vec<_> = resources
                .iter()
                .filter(|r| r.media_type == "text/css")
                .collect();

            let css_content = if css_resources.is_empty() {
                default_stylesheet().to_string()
            } else {
                // Concatenate all CSS resources into a single style.css
                let mut combined = String::new();
                for res in &css_resources {
                    if let Ok(text) = std::str::from_utf8(res.data) {
                        if !combined.is_empty() {
                            combined.push('\n');
                        }
                        combined.push_str(text);
                    }
                }
                if combined.is_empty() {
                    default_stylesheet().to_string()
                } else {
                    combined
                }
            };

            // 1. Write index.html (HTML content with stylesheet link)
            let html = generate_htmlz_content(book);
            zip.start_file("index.html", options)
                .map_err(|e| EruditioError::Format(format!("Failed to write index.html: {}", e)))?;
            zip.write_all(html.as_bytes())?;

            // 2. Write metadata.opf
            let opf = generate_htmlz_opf(book);
            zip.start_file("metadata.opf", options)
                .map_err(|e| {
                    EruditioError::Format(format!("Failed to write metadata.opf: {}", e))
                })?;
            zip.write_all(opf.as_bytes())?;

            // 3. Write style.css
            zip.start_file("style.css", options)
                .map_err(|e| {
                    EruditioError::Format(format!("Failed to write style.css: {}", e))
                })?;
            zip.write_all(css_content.as_bytes())?;

            // 4. Write image resources
            for res in &resources {
                if res.media_type.starts_with("image/") {
                    let filename = res.href.rsplit('/').next().unwrap_or(res.href);
                    let path = format!("images/{}", filename);
                    zip.start_file(&path, options).map_err(|e| {
                        EruditioError::Format(format!("Failed to write {}: {}", path, e))
                    })?;
                    zip.write_all(res.data)?;
                }
            }

            zip.finish()
                .map_err(|e| EruditioError::Format(format!("Failed to finalize HTMLZ: {}", e)))?;
        }

        output.write_all(zip_buf.get_ref())?;
        Ok(())
    }
}

/// Generates HTML content for the HTMLZ archive (without data URI embedding).
/// Includes a `<link>` to `style.css` in the `<head>`.
fn generate_htmlz_content(book: &Book) -> String {
    let title = book.metadata.title.as_deref().unwrap_or("Untitled");
    let chapters = book.chapters();

    let mut body = String::with_capacity(4096);
    for (i, chapter) in chapters.iter().enumerate() {
        if i > 0 {
            body.push_str("<hr />\n");
        }
        if let Some(ref ch_title) = chapter.title {
            body.push_str(&format!("<h1>{}</h1>\n", escape_html(ch_title)));
        }
        let content = match chapter.title {
            Some(ref t) => strip_leading_heading(&chapter.content, t),
            None => &chapter.content,
        };
        body.push_str(content);
        body.push('\n');
    }

    // DO NOT embed resources as data URIs — they are written as separate ZIP entries

    let mut html =
        crate::formats::html::parser::build_html_document(title, &book.metadata, &body);

    // Inject stylesheet link into <head> (before </head>)
    if let Some(pos) = html.find("</head>") {
        html.insert_str(pos, "<link rel=\"stylesheet\" href=\"style.css\">\n");
    }

    html
}

/// Generates a simplified OPF metadata document for the HTMLZ archive.
fn generate_htmlz_opf(book: &Book) -> String {
    let m = &book.metadata;
    let mut xml = String::with_capacity(1024);

    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"2.0\">\n");
    xml.push_str("  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">\n");

    if let Some(ref title) = m.title {
        xml.push_str("    <dc:title>");
        xml.push_str(&escape_html(title));
        xml.push_str("</dc:title>\n");
    }
    for author in &m.authors {
        xml.push_str("    <dc:creator>");
        xml.push_str(&escape_html(author));
        xml.push_str("</dc:creator>\n");
    }
    if let Some(ref lang) = m.language {
        xml.push_str("    <dc:language>");
        xml.push_str(&escape_html(lang));
        xml.push_str("</dc:language>\n");
    }
    if let Some(ref publisher) = m.publisher {
        xml.push_str("    <dc:publisher>");
        xml.push_str(&escape_html(publisher));
        xml.push_str("</dc:publisher>\n");
    }
    if let Some(ref identifier) = m.identifier {
        xml.push_str("    <dc:identifier>");
        xml.push_str(&escape_html(identifier));
        xml.push_str("</dc:identifier>\n");
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
    xml.push_str("</package>\n");
    xml
}

/// Finds the first HTML file in a ZIP archive.
fn find_html_file<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Option<String> {
    for i in 0..archive.len() {
        if let Ok(file) = archive.by_index(i) {
            let name = file.name().to_lowercase();
            if name.ends_with(".html") || name.ends_with(".htm") || name.ends_with(".xhtml") {
                return Some(file.name().to_string());
            }
        }
    }
    None
}

/// Parses OPF XML and merges Dublin Core metadata into the book.
///
/// Uses quick-xml for lightweight event-based parsing. Only overwrites
/// fields that are not already set from the HTML `<head>` metadata,
/// except for authors which are always taken from OPF if present.
fn merge_opf_metadata(opf_xml: &str, book: &mut Book) {
    let mut reader = XmlReader::from_str(opf_xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut current_tag = String::new();
    let mut current_text = String::new();
    let mut in_metadata = false;
    let mut current_scheme: Option<String> = None;
    let mut opf_authors: Vec<String> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = e.name();
                let tag = xml_utils::local_tag_name(name.as_ref());
                if tag == "metadata" {
                    in_metadata = true;
                } else if in_metadata {
                    current_tag = tag.to_string();
                    current_text.clear();
                    current_scheme = None;

                    // Track the opf:scheme attribute on <dc:identifier>
                    if tag == "identifier" {
                        current_scheme = xml_utils::get_attribute(e, "opf:scheme")
                            .or_else(|| xml_utils::get_attribute(e, "scheme"));
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                let name = e.name();
                let tag = xml_utils::local_tag_name(name.as_ref());
                // Handle self-closing <meta name="..." content="..."/> elements
                if in_metadata && tag == "meta" {
                    let name = xml_utils::get_attribute(e, "name");
                    let content = xml_utils::get_attribute(e, "content");
                    if let (Some(name), Some(content)) = (name, content) {
                        match name.as_str() {
                            "calibre:series" => {
                                if book.metadata.series.is_none() {
                                    book.metadata.series = Some(content);
                                }
                            }
                            "calibre:series_index" => {
                                if book.metadata.series_index.is_none() {
                                    if let Ok(idx) = content.parse::<f64>() {
                                        book.metadata.series_index = Some(idx);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_metadata && !current_tag.is_empty() {
                    current_text = xml_utils::bytes_to_string(e.as_ref());
                }
            }
            Ok(Event::End(ref e)) => {
                let name = e.name();
                let tag = xml_utils::local_tag_name(name.as_ref());
                if tag == "metadata" {
                    in_metadata = false;
                } else if in_metadata && !current_tag.is_empty() {
                    let text = current_text.trim().to_string();
                    if !text.is_empty() {
                        match current_tag.as_str() {
                            "title" => {
                                if book.metadata.title.is_none() {
                                    book.metadata.title = Some(text);
                                }
                            }
                            "creator" => {
                                opf_authors.push(text);
                            }
                            "language" => {
                                if book.metadata.language.is_none() {
                                    book.metadata.language = Some(text);
                                }
                            }
                            "publisher" => {
                                if book.metadata.publisher.is_none() {
                                    book.metadata.publisher = Some(text);
                                }
                            }
                            "identifier" => {
                                if let Some(ref scheme) = current_scheme {
                                    if scheme.eq_ignore_ascii_case("ISBN") {
                                        if book.metadata.isbn.is_none() {
                                            book.metadata.isbn = Some(text);
                                        }
                                    }
                                } else if book.metadata.identifier.is_none() {
                                    book.metadata.identifier = Some(text);
                                }
                            }
                            "description" => {
                                if book.metadata.description.is_none() {
                                    book.metadata.description = Some(text);
                                }
                            }
                            "subject" => {
                                if !book.metadata.subjects.contains(&text) {
                                    book.metadata.subjects.push(text);
                                }
                            }
                            "rights" => {
                                if book.metadata.rights.is_none() {
                                    book.metadata.rights = Some(text);
                                }
                            }
                            _ => {}
                        }
                    }
                    current_tag.clear();
                    current_text.clear();
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    // Authors from OPF take precedence if the HTML-parsed ones are empty.
    if book.metadata.authors.is_empty() && !opf_authors.is_empty() {
        book.metadata.authors = opf_authors;
    }
}

/// Returns a minimal default stylesheet matching calibre's HTMLZ output behavior.
fn default_stylesheet() -> &'static str {
    "\
body {
  margin: 5%;
  font-family: serif;
  line-height: 1.6;
}
h1, h2, h3 {
  font-family: sans-serif;
  margin-top: 1.5em;
  margin-bottom: 0.5em;
}
h1 { font-size: 1.8em; }
h2 { font-size: 1.4em; }
h3 { font-size: 1.2em; }
p { margin: 0.5em 0; text-indent: 1.5em; }
"
}

/// Guesses MIME type from a filename extension.
fn guess_media_type(filename: &str) -> &'static str {
    match filename.rsplit('.').next().map(|e| e.to_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("css") => "text/css",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;

    #[test]
    fn htmlz_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("HTMLZ Test".into());
        book.metadata.authors.push("Test Author".into());
        book.add_chapter(&Chapter {
            title: Some("Section 1".into()),
            content: "<p>Hello from HTMLZ</p>".into(),
            id: Some("s1".into()),
        });

        // Write
        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        // Read back
        let mut cursor = Cursor::new(output);
        let decoded = HtmlzReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("HTMLZ Test"));
        let chapters = decoded.chapters();
        assert!(!chapters.is_empty());
    }

    #[test]
    fn htmlz_preserves_metadata() {
        let mut book = Book::new();
        book.metadata.title = Some("Meta Test".into());
        book.metadata.authors.push("Alice".into());
        book.metadata.language = Some("en".into());
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        let mut cursor = Cursor::new(output);
        let decoded = HtmlzReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("Meta Test"));
        assert_eq!(decoded.metadata.authors, vec!["Alice"]);
        assert_eq!(decoded.metadata.language.as_deref(), Some("en"));
    }

    #[test]
    fn htmlz_writer_includes_metadata_opf() {
        let mut book = Book::new();
        book.metadata.title = Some("OPF Test".into());
        book.metadata.authors.push("Bob".into());
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>Hi</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        // Verify the ZIP contains metadata.opf
        let cursor = Cursor::new(output);
        let mut archive = ZipArchive::new(cursor).unwrap();
        let mut opf = archive.by_name("metadata.opf").expect("metadata.opf should exist");
        let mut opf_content = String::new();
        opf.read_to_string(&mut opf_content).unwrap();

        assert!(opf_content.contains("<dc:title>OPF Test</dc:title>"));
        assert!(opf_content.contains("<dc:creator>Bob</dc:creator>"));
    }

    #[test]
    fn htmlz_writer_includes_images() {
        let mut book = Book::new();
        book.metadata.title = Some("Image Test".into());
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>text</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_resource("img1", "images/cover.png", vec![0x89, 0x50, 0x4E, 0x47], "image/png");

        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        // Verify the ZIP contains the image
        let cursor = Cursor::new(output);
        let mut archive = ZipArchive::new(cursor).unwrap();
        let mut img = archive
            .by_name("images/cover.png")
            .expect("images/cover.png should exist");
        let mut img_data = Vec::new();
        img.read_to_end(&mut img_data).unwrap();
        assert_eq!(img_data, vec![0x89, 0x50, 0x4E, 0x47]);
    }

    #[test]
    fn htmlz_metadata_opf_content() {
        let mut book = Book::new();
        book.metadata.title = Some("Full Meta".into());
        book.metadata.authors.push("Author A".into());
        book.metadata.authors.push("Author B".into());
        book.metadata.language = Some("fr".into());
        book.metadata.publisher = Some("Publisher X".into());
        book.metadata.isbn = Some("978-0-123456-78-9".into());
        book.metadata.description = Some("A test book".into());
        book.metadata.subjects.push("Fiction".into());
        book.metadata.rights = Some("CC BY 4.0".into());
        book.metadata.series = Some("Test Series".into());
        book.metadata.series_index = Some(3.0);
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        let opf = generate_htmlz_opf(&book);

        assert!(opf.contains("<dc:title>Full Meta</dc:title>"));
        assert!(opf.contains("<dc:creator>Author A</dc:creator>"));
        assert!(opf.contains("<dc:creator>Author B</dc:creator>"));
        assert!(opf.contains("<dc:language>fr</dc:language>"));
        assert!(opf.contains("<dc:publisher>Publisher X</dc:publisher>"));
        assert!(opf.contains("opf:scheme=\"ISBN\">978-0-123456-78-9</dc:identifier>"));
        assert!(opf.contains("<dc:description>A test book</dc:description>"));
        assert!(opf.contains("<dc:subject>Fiction</dc:subject>"));
        assert!(opf.contains("<dc:rights>CC BY 4.0</dc:rights>"));
        assert!(opf.contains("calibre:series\" content=\"Test Series\""));
        assert!(opf.contains("calibre:series_index\" content=\"3\""));
    }

    #[test]
    fn htmlz_round_trip_with_metadata_from_opf() {
        let mut book = Book::new();
        book.metadata.title = Some("OPF Round Trip".into());
        book.metadata.authors.push("Jane".into());
        book.metadata.language = Some("de".into());
        book.metadata.publisher = Some("Verlag".into());
        book.metadata.description = Some("Ein Buch".into());
        book.add_chapter(&Chapter {
            title: Some("Kapitel 1".into()),
            content: "<p>Inhalt</p>".into(),
            id: Some("ch1".into()),
        });

        // Write
        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        // Read back
        let mut cursor = Cursor::new(output);
        let decoded = HtmlzReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("OPF Round Trip"));
        assert_eq!(decoded.metadata.authors, vec!["Jane"]);
        assert_eq!(decoded.metadata.language.as_deref(), Some("de"));
        assert_eq!(decoded.metadata.publisher.as_deref(), Some("Verlag"));
        assert_eq!(decoded.metadata.description.as_deref(), Some("Ein Buch"));
    }

    #[test]
    fn htmlz_round_trip_with_images() {
        let mut book = Book::new();
        book.metadata.title = Some("Image Round Trip".into());
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>text with image</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_resource(
            "cover",
            "images/cover.jpg",
            vec![0xFF, 0xD8, 0xFF, 0xE0],
            "image/jpeg",
        );
        book.add_resource(
            "fig1",
            "images/figure1.png",
            vec![0x89, 0x50, 0x4E, 0x47],
            "image/png",
        );

        // Write
        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        // Read back
        let mut cursor = Cursor::new(output);
        let decoded = HtmlzReader::new().read_book(&mut cursor).unwrap();

        let resources = decoded.resources();
        let image_resources: Vec<_> = resources
            .iter()
            .filter(|r| r.media_type.starts_with("image/"))
            .collect();
        assert_eq!(image_resources.len(), 2, "Should have 2 image resources");

        // Check data is preserved (order may vary)
        let jpeg_res = resources.iter().find(|r| r.media_type == "image/jpeg");
        let png_res = resources.iter().find(|r| r.media_type == "image/png");
        assert!(jpeg_res.is_some(), "Should have JPEG resource");
        assert!(png_res.is_some(), "Should have PNG resource");
        assert_eq!(jpeg_res.unwrap().data, &[0xFF, 0xD8, 0xFF, 0xE0]);
        assert_eq!(png_res.unwrap().data, &[0x89, 0x50, 0x4E, 0x47]);
    }

    #[test]
    fn htmlz_backward_compat_no_opf() {
        // Create a minimal HTMLZ with only index.html (no metadata.opf)
        let html = b"<!DOCTYPE html>\n<html>\n<head>\n<title>Legacy</title>\n</head>\n<body>\n<p>Old content</p>\n</body>\n</html>\n";

        let mut zip_buf = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut zip_buf);
            let options: FileOptions<'_, ()> =
                FileOptions::default().compression_method(CompressionMethod::Deflated);
            zip.start_file("index.html", options).unwrap();
            zip.write_all(html).unwrap();
            zip.finish().unwrap();
        }

        let mut cursor = Cursor::new(zip_buf.into_inner());
        let decoded = HtmlzReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("Legacy"));
        let chapters = decoded.chapters();
        assert!(!chapters.is_empty());
        assert!(chapters[0].content.contains("Old content"));
    }

    #[test]
    fn htmlz_html_does_not_contain_data_uris() {
        let mut book = Book::new();
        book.metadata.title = Some("No Data URI".into());
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>text</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_resource("img1", "cover.png", vec![0x89, 0x50], "image/png");

        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        // Read the index.html from the zip
        let cursor = Cursor::new(output);
        let mut archive = ZipArchive::new(cursor).unwrap();
        let mut html_file = archive.by_name("index.html").unwrap();
        let mut html_content = String::new();
        html_file.read_to_string(&mut html_content).unwrap();

        assert!(
            !html_content.contains("data:image/"),
            "HTML should not contain data URI images"
        );
        assert!(
            !html_content.contains("base64,"),
            "HTML should not contain base64 data"
        );
    }

    #[test]
    fn generate_htmlz_opf_minimal() {
        let book = Book::new();
        let opf = generate_htmlz_opf(&book);

        assert!(opf.contains("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(opf.contains("<package xmlns=\"http://www.idpf.org/2007/opf\""));
        assert!(opf.contains("<metadata"));
        assert!(opf.contains("</metadata>"));
        assert!(opf.contains("</package>"));
    }

    #[test]
    fn merge_opf_metadata_parses_correctly() {
        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>OPF Title</dc:title>
    <dc:creator>OPF Author</dc:creator>
    <dc:language>es</dc:language>
    <dc:publisher>OPF Publisher</dc:publisher>
    <dc:description>OPF Description</dc:description>
    <dc:identifier opf:scheme="ISBN">978-1234567890</dc:identifier>
    <dc:subject>Science</dc:subject>
    <dc:rights>Public Domain</dc:rights>
  </metadata>
</package>"#;

        let mut book = Book::new();
        merge_opf_metadata(opf, &mut book);

        assert_eq!(book.metadata.title.as_deref(), Some("OPF Title"));
        assert_eq!(book.metadata.authors, vec!["OPF Author"]);
        assert_eq!(book.metadata.language.as_deref(), Some("es"));
        assert_eq!(book.metadata.publisher.as_deref(), Some("OPF Publisher"));
        assert_eq!(
            book.metadata.description.as_deref(),
            Some("OPF Description")
        );
        assert_eq!(book.metadata.isbn.as_deref(), Some("978-1234567890"));
        assert_eq!(book.metadata.subjects, vec!["Science"]);
        assert_eq!(book.metadata.rights.as_deref(), Some("Public Domain"));
    }

    #[test]
    fn merge_opf_does_not_overwrite_html_metadata() {
        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>OPF Title</dc:title>
    <dc:creator>OPF Author</dc:creator>
    <dc:language>es</dc:language>
  </metadata>
</package>"#;

        let mut book = Book::new();
        // Pre-fill from HTML parsing
        book.metadata.title = Some("HTML Title".into());
        book.metadata.language = Some("en".into());

        merge_opf_metadata(opf, &mut book);

        // HTML values should be preserved
        assert_eq!(book.metadata.title.as_deref(), Some("HTML Title"));
        assert_eq!(book.metadata.language.as_deref(), Some("en"));
        // Authors from OPF should be used since HTML had none
        assert_eq!(book.metadata.authors, vec!["OPF Author"]);
    }

    #[test]
    fn htmlz_writer_includes_style_css_from_manifest() {
        let mut book = Book::new();
        book.metadata.title = Some("CSS Test".into());
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>Styled content</p>".into(),
            id: Some("ch1".into()),
        });
        let css = b"body { color: red; }";
        book.add_resource("my-style", "styles/main.css", css.to_vec(), "text/css");

        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        // Verify the ZIP contains style.css with the manifest CSS content
        let cursor = Cursor::new(output);
        let mut archive = ZipArchive::new(cursor).unwrap();
        let mut css_file = archive.by_name("style.css").expect("style.css should exist");
        let mut css_content = String::new();
        css_file.read_to_string(&mut css_content).unwrap();
        assert!(
            css_content.contains("body { color: red; }"),
            "style.css should contain manifest CSS"
        );
    }

    #[test]
    fn htmlz_writer_generates_default_stylesheet_when_no_css() {
        let mut book = Book::new();
        book.metadata.title = Some("No CSS".into());
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>Plain content</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        // Verify the ZIP contains style.css with default stylesheet
        let cursor = Cursor::new(output);
        let mut archive = ZipArchive::new(cursor).unwrap();
        let mut css_file = archive.by_name("style.css").expect("style.css should exist");
        let mut css_content = String::new();
        css_file.read_to_string(&mut css_content).unwrap();
        assert!(
            css_content.contains("font-family: serif"),
            "Default stylesheet should contain basic font styling"
        );
        assert!(
            css_content.contains("margin: 5%"),
            "Default stylesheet should contain body margins"
        );
    }

    #[test]
    fn htmlz_index_html_contains_stylesheet_link() {
        let mut book = Book::new();
        book.metadata.title = Some("Link Test".into());
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        let cursor = Cursor::new(output);
        let mut archive = ZipArchive::new(cursor).unwrap();
        let mut html_file = archive.by_name("index.html").unwrap();
        let mut html_content = String::new();
        html_file.read_to_string(&mut html_content).unwrap();

        assert!(
            html_content.contains(r#"<link rel="stylesheet" href="style.css">"#),
            "index.html should contain a link to style.css"
        );
    }

    #[test]
    fn htmlz_reader_loads_style_css() {
        let mut book = Book::new();
        book.metadata.title = Some("CSS Round Trip".into());
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>Styled</p>".into(),
            id: Some("ch1".into()),
        });
        let css = b"h1 { font-size: 2em; }";
        book.add_resource("custom-css", "custom.css", css.to_vec(), "text/css");

        // Write
        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        // Read back
        let mut cursor = Cursor::new(output);
        let decoded = HtmlzReader::new().read_book(&mut cursor).unwrap();

        let resources = decoded.resources();
        let css_res = resources.iter().find(|r| r.media_type == "text/css");
        assert!(css_res.is_some(), "Should have CSS resource after reading");
        assert!(
            std::str::from_utf8(css_res.unwrap().data)
                .unwrap()
                .contains("font-size: 2em"),
            "CSS content should be preserved"
        );
    }
}
