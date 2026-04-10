//! OEB (Open eBook) format reader and writer.
//!
//! OEB is an exploded EPUB-like format: a ZIP archive containing an OPF
//! package document, XHTML content files, and resources. It is the precursor
//! to EPUB and uses the same OPF/Dublin Core metadata model.

use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use crate::formats::common::text_utils;
use crate::formats::common::text_utils::push_escape_xml;
use crate::formats::common::zip_utils::ZIP_DEFLATE_LEVEL;
use std::borrow::Cow;
use std::fmt::Write as FmtWrite;
use std::io::{Cursor, Read, Write};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// OEB format reader.
///
/// Reads a ZIP archive containing an OPF package document and XHTML content.
#[derive(Default)]
pub struct OebReader;

impl OebReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for OebReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        let cursor = Cursor::new(data);

        let mut archive = ZipArchive::new(cursor)?;

        // Find the OPF file.
        let opf_name = find_opf_file(&archive)
            .ok_or_else(|| EruditioError::Format("No OPF file found in OEB archive".into()))?;

        // Read and parse the OPF.
        let opf_content = read_zip_entry(&mut archive, &opf_name)?;
        let opf_str = crate::formats::common::text_utils::bytes_to_cow_str(&opf_content);

        let mut book = Book::new();

        // Extract metadata from OPF.
        parse_opf_metadata(&opf_str, &mut book);

        // Extract spine items (ordered content references).
        let manifest = parse_opf_manifest(&opf_str);
        let spine_idrefs = parse_opf_spine(&opf_str);

        // Determine base path of the OPF for resolving relative hrefs.
        let opf_base = opf_name
            .rfind('/')
            .map(|i| &opf_name[..i + 1])
            .unwrap_or("");

        // Read content files in spine order.
        let ordered_hrefs: Vec<String> = if spine_idrefs.is_empty() {
            // No spine — use all HTML manifest items in order.
            manifest
                .iter()
                .filter(|(_, href, mt)| is_content_type(mt) || is_html_href(href))
                .map(|(_, href, _)| href.clone())
                .collect()
        } else {
            spine_idrefs
                .iter()
                .filter_map(|idref| {
                    manifest
                        .iter()
                        .find(|(id, _, _)| id == idref)
                        .map(|(_, href, _)| href.clone())
                })
                .collect()
        };

        let mut full_path = String::with_capacity(opf_base.len() + 64);
        for (i, href) in ordered_hrefs.iter().enumerate() {
            full_path.clear();
            full_path.push_str(opf_base);
            full_path.push_str(href);
            match read_zip_entry(&mut archive, &full_path) {
                Ok(content_bytes) => {
                    let content = String::from_utf8(content_bytes)
                        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
                    let title = extract_title(&content);
                    // Extract <body> content if present, else use full content.
                    let body = match extract_body(&content) {
                        Some(b) => b.to_string(),
                        None => content, // move, no copy
                    };

                    book.add_chapter(Chapter {
                        title,
                        content: body,
                        id: Some(format!("oeb_ch_{}", i)),
                    });
                },
                Err(_) => continue, // Skip missing files.
            }
        }

        // If no chapters found, try reading all HTML files.
        if book.chapter_count() == 0 {
            for i in 0..archive.len() {
                let name = match archive.by_index(i) {
                    Ok(f) => f.name().to_string(),
                    Err(_) => continue,
                };
                if (text_utils::ends_with_ascii_ci(&name, ".html")
                    || text_utils::ends_with_ascii_ci(&name, ".htm")
                    || text_utils::ends_with_ascii_ci(&name, ".xhtml"))
                    && let Ok(bytes) = read_zip_entry(&mut archive, &name)
                {
                    let content = String::from_utf8(bytes)
                        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
                    let body = match extract_body(&content) {
                        Some(b) => b.to_string(),
                        None => content,
                    };
                    book.add_chapter(Chapter {
                        title: None,
                        content: body,
                        id: Some(format!("oeb_file_{}", i)),
                    });
                }
            }
        }

        if book.chapter_count() == 0 {
            book.add_chapter(Chapter {
                title: book.metadata.title.clone(),
                content: "<p></p>".into(),
                id: Some("oeb_empty".into()),
            });
        }

        // Extract image resources.
        let image_entries: Vec<String> = (0..archive.len())
            .filter_map(|i| {
                let f = archive.by_index(i).ok()?;
                let name = f.name().to_string();
                if text_utils::ends_with_ascii_ci(&name, ".jpg")
                    || text_utils::ends_with_ascii_ci(&name, ".jpeg")
                    || text_utils::ends_with_ascii_ci(&name, ".png")
                    || text_utils::ends_with_ascii_ci(&name, ".gif")
                    || text_utils::ends_with_ascii_ci(&name, ".svg")
                {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();

        for (idx, name) in image_entries.iter().enumerate() {
            if let Ok(img_data) = read_zip_entry(&mut archive, name) {
                let media_type = guess_media_type(name);
                let id = format!("oeb_img_{}", idx);
                book.add_resource(&id, name, img_data, media_type);
            }
        }

        Ok(book)
    }
}

/// OEB format writer.
///
/// Produces a ZIP archive containing an OPF package document and XHTML
/// content files for each chapter.
#[derive(Default)]
pub struct OebWriter;

impl OebWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for OebWriter {
    fn write_book(&self, book: &Book, output: &mut dyn Write) -> Result<()> {
        let mut zip_buf = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut zip_buf);
            let deflated: FileOptions<'_, ()> = FileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .compression_level(ZIP_DEFLATE_LEVEL);
            let stored: FileOptions<'_, ()> = FileOptions::default()
                .compression_method(CompressionMethod::Stored);

            /// Minimum entry size for using Deflated compression.
            const MIN_DEFLATE_SIZE: usize = 4096;

            let chapters = book.chapter_views();
            let resources = book.resources();

            // Generate XHTML files for each chapter.
            let mut content_items: Vec<(String, String)> = Vec::new(); // (id, filename)

            for (i, chapter) in chapters.iter().enumerate() {
                let filename = format!("content/chapter_{}.xhtml", i);
                let id = format!("chapter_{}", i);
                let fallback_title = format!("Chapter {}", i + 1);
                let title = chapter.title.as_deref().unwrap_or(&fallback_title);

                let mut xhtml = String::with_capacity(256 + chapter.content.len());
                xhtml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                     <!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.1//EN\" \
                     \"http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd\">\n\
                     <html xmlns=\"http://www.w3.org/1999/xhtml\">\n\
                     <head><title>");
                push_escape_xml(&mut xhtml, title);
                xhtml.push_str("</title></head>\n\
                     <body>\n");
                xhtml.push_str(&chapter.content);
                xhtml.push_str("\n</body>\n</html>");

                let opts = if xhtml.len() >= MIN_DEFLATE_SIZE { deflated } else { stored };
                zip.start_file(&filename, opts)?;
                zip.write_all(xhtml.as_bytes())?;

                content_items.push((id, filename));
            }

            // Write image resources (Stored — images are already compressed).
            let mut resource_items: Vec<(String, String, &str)> = Vec::new();

            for (i, resource) in resources.iter().enumerate() {
                let fallback_name = format!("image_{}", i);
                let basename = resource.href.rsplit('/').next().unwrap_or(&fallback_name);
                let filename = format!("images/{}", basename);
                let id = format!("image_{}", i);

                zip.start_file(&filename, stored)?;
                zip.write_all(resource.data)?;

                resource_items.push((id, filename, resource.media_type));
            }

            // Generate OPF.
            let opf = build_opf(book, &content_items, &resource_items);
            let opf_opts = if opf.len() >= MIN_DEFLATE_SIZE { deflated } else { stored };
            zip.start_file("content.opf", opf_opts)?;
            zip.write_all(opf.as_bytes())?;

            zip.finish()?;
        }

        output.write_all(zip_buf.get_ref())?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// OPF generation
// ---------------------------------------------------------------------------

/// Builds an OPF 2.0 package document.
fn build_opf(
    book: &Book,
    content_items: &[(String, String)],
    resource_items: &[(String, String, &str)],
) -> String {
    let title = book.metadata.title.as_deref().unwrap_or("Untitled");
    let language = book.metadata.language.as_deref().unwrap_or("en");

    let mut opf = String::with_capacity(2048);
    opf.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    opf.push_str("<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"2.0\" unique-identifier=\"uid\">\n");

    // Metadata.
    opf.push_str("  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n");
    opf.push_str("    <dc:title>");
    push_escape_xml(&mut opf, title);
    opf.push_str("</dc:title>\n");
    opf.push_str("    <dc:language>");
    push_escape_xml(&mut opf, language);
    opf.push_str("</dc:language>\n");
    for author in &book.metadata.authors {
        opf.push_str("    <dc:creator>");
        push_escape_xml(&mut opf, author);
        opf.push_str("</dc:creator>\n");
    }
    if let Some(ref desc) = book.metadata.description {
        opf.push_str("    <dc:description>");
        push_escape_xml(&mut opf, desc);
        opf.push_str("</dc:description>\n");
    }
    if let Some(ref publisher) = book.metadata.publisher {
        opf.push_str("    <dc:publisher>");
        push_escape_xml(&mut opf, publisher);
        opf.push_str("</dc:publisher>\n");
    }
    if let Some(ref isbn) = book.metadata.isbn {
        opf.push_str("    <dc:identifier id=\"uid\">");
        push_escape_xml(&mut opf, isbn);
        opf.push_str("</dc:identifier>\n");
    } else {
        opf.push_str("    <dc:identifier id=\"uid\">eruditio-oeb-export</dc:identifier>\n");
    }
    opf.push_str("  </metadata>\n");

    // Manifest.
    opf.push_str("  <manifest>\n");
    for (id, filename) in content_items {
        opf.push_str("    <item id=\"");
        push_escape_xml(&mut opf, id);
        opf.push_str("\" href=\"");
        push_escape_xml(&mut opf, filename);
        opf.push_str("\" media-type=\"application/xhtml+xml\" />\n");
    }
    for (id, filename, media_type) in resource_items {
        opf.push_str("    <item id=\"");
        push_escape_xml(&mut opf, id);
        opf.push_str("\" href=\"");
        push_escape_xml(&mut opf, filename);
        opf.push_str("\" media-type=\"");
        push_escape_xml(&mut opf, media_type);
        opf.push_str("\" />\n");
    }
    opf.push_str("  </manifest>\n");

    // Spine.
    opf.push_str("  <spine>\n");
    for (id, _) in content_items {
        opf.push_str("    <itemref idref=\"");
        push_escape_xml(&mut opf, id);
        opf.push_str("\" />\n");
    }
    opf.push_str("  </spine>\n");

    opf.push_str("</package>\n");
    opf
}

// ---------------------------------------------------------------------------
// OPF parsing (simple regex-free XML parsing)
// ---------------------------------------------------------------------------

/// Extracts metadata from an OPF string.
fn parse_opf_metadata(opf: &str, book: &mut Book) {
    // Title: <dc:title>...</dc:title>
    if let Some(title) = extract_tag_content(opf, "dc:title") {
        book.metadata.title = Some(title);
    }

    // Language: <dc:language>...</dc:language>
    if let Some(lang) = extract_tag_content(opf, "dc:language") {
        book.metadata.language = Some(lang);
    }

    // Authors: <dc:creator>...</dc:creator>
    for author in extract_all_tag_contents(opf, "dc:creator") {
        if !author.is_empty() {
            book.metadata.authors.push(author);
        }
    }

    // Description: <dc:description>...</dc:description>
    if let Some(desc) = extract_tag_content(opf, "dc:description") {
        book.metadata.description = Some(desc);
    }

    // Publisher: <dc:publisher>...</dc:publisher>
    if let Some(pub_name) = extract_tag_content(opf, "dc:publisher") {
        book.metadata.publisher = Some(pub_name);
    }

    // Identifier: <dc:identifier>...</dc:identifier>
    if let Some(id) = extract_tag_content(opf, "dc:identifier") {
        book.metadata.isbn = Some(id);
    }

    // Subjects: <dc:subject>...</dc:subject>
    for subject in extract_all_tag_contents(opf, "dc:subject") {
        if !subject.is_empty() {
            book.metadata.subjects.push(subject);
        }
    }
}

/// Parses the OPF manifest into (id, href, media-type) tuples.
fn parse_opf_manifest(opf: &str) -> Vec<(String, String, String)> {
    let mut items = Vec::new();

    // Find <manifest>...</manifest> section.
    let manifest_section = match extract_section(opf, "manifest") {
        Some(s) => s,
        None => return items,
    };

    // Parse each <item ... /> in the manifest.
    let mut search_from = 0;
    while let Some(start) = manifest_section[search_from..].find("<item ") {
        let abs_start = search_from + start;
        let end = match manifest_section[abs_start..].find("/>") {
            Some(e) => abs_start + e + 2,
            None => match manifest_section[abs_start..].find("</item>") {
                Some(e) => abs_start + e + 7,
                None => break,
            },
        };

        let item_tag = &manifest_section[abs_start..end];
        let id = extract_attr(item_tag, "id").unwrap_or_default();
        let href = extract_attr(item_tag, "href").unwrap_or_default();
        let media_type = extract_attr(item_tag, "media-type").unwrap_or_default();

        if !id.is_empty() && !href.is_empty() {
            items.push((id, href, media_type));
        }

        search_from = end;
    }

    items
}

/// Parses the OPF spine and returns ordered idref values.
fn parse_opf_spine(opf: &str) -> Vec<String> {
    let mut idrefs = Vec::new();

    let spine_section = match extract_section(opf, "spine") {
        Some(s) => s,
        None => return idrefs,
    };

    let mut search_from = 0;
    while let Some(start) = spine_section[search_from..].find("<itemref ") {
        let abs_start = search_from + start;
        let end = match spine_section[abs_start..].find("/>") {
            Some(e) => abs_start + e + 2,
            None => match spine_section[abs_start..].find('>') {
                Some(e) => abs_start + e + 1,
                None => break,
            },
        };

        let tag = &spine_section[abs_start..end];
        if let Some(idref) = extract_attr(tag, "idref") {
            idrefs.push(idref);
        }

        search_from = end;
    }

    idrefs
}

// ---------------------------------------------------------------------------
// XML/HTML helpers
// ---------------------------------------------------------------------------

/// Extracts text content of the first occurrence of `<tag>...</tag>`.
fn extract_tag_content(xml: &str, tag: &str) -> Option<String> {
    let mut open = String::with_capacity(1 + tag.len());
    let _ = write!(open, "<{}", tag);
    let mut close = String::with_capacity(3 + tag.len());
    let _ = write!(close, "</{}>", tag);

    let start = xml.find(&open)?;
    let content_start = xml[start..].find('>')? + start + 1;
    let content_end = xml[content_start..].find(&close)? + content_start;

    let text = xml[content_start..content_end].trim();
    if text.is_empty() {
        None
    } else {
        Some(unescape_xml(text).into_owned())
    }
}

/// Extracts text content of all occurrences of `<tag>...</tag>`.
fn extract_all_tag_contents(xml: &str, tag: &str) -> Vec<String> {
    let mut results = Vec::new();
    let mut open = String::with_capacity(1 + tag.len());
    let _ = write!(open, "<{}", tag);
    let mut close = String::with_capacity(3 + tag.len());
    let _ = write!(close, "</{}>", tag);

    let mut search_from = 0;
    while let Some(start) = xml[search_from..].find(&open) {
        let abs_start = search_from + start;
        let content_start = match xml[abs_start..].find('>') {
            Some(e) => abs_start + e + 1,
            None => break,
        };
        let content_end = match xml[content_start..].find(&close) {
            Some(e) => content_start + e,
            None => break,
        };

        let text = xml[content_start..content_end].trim();
        if !text.is_empty() {
            results.push(unescape_xml(text).into_owned());
        }

        search_from = content_end + close.len();
    }

    results
}

/// Extracts a section between `<tag...>` and `</tag>`.
fn extract_section<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let mut open = String::with_capacity(1 + tag.len());
    let _ = write!(open, "<{}", tag);
    let mut close = String::with_capacity(3 + tag.len());
    let _ = write!(close, "</{}>", tag);

    let start = xml.find(&open)?;
    let content_start = xml[start..].find('>')? + start + 1;
    let content_end = xml[content_start..].find(&close)? + content_start;

    Some(&xml[content_start..content_end])
}

/// Extracts the value of an XML attribute from a tag string.
fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let patterns = [format!("{}=\"", attr), format!("{}='", attr)];

    for pat in &patterns {
        if let Some(start) = tag.find(pat.as_str()) {
            let value_start = start + pat.len();
            let quote = tag.as_bytes()[start + pat.len() - 1] as char;
            if let Some(end) = tag[value_start..].find(quote) {
                return Some(unescape_xml(&tag[value_start..value_start + end]).into_owned());
            }
        }
    }
    None
}

/// Extracts the <body> content from an XHTML document.
fn extract_body(html: &str) -> Option<&str> {
    let bytes = html.as_bytes();
    let body_start = text_utils::find_case_insensitive(bytes, b"<body")?;
    let content_start = html[body_start..].find('>')? + body_start + 1;
    let content_end =
        text_utils::find_case_insensitive(&bytes[content_start..], b"</body>")? + content_start;

    let body = html[content_start..content_end].trim();
    if body.is_empty() { None } else { Some(body) }
}

/// Extracts the <title> content from an XHTML document.
fn extract_title(html: &str) -> Option<String> {
    extract_tag_content(html, "title")
}

/// Finds the OPF file in the archive.
fn find_opf_file<R: Read + std::io::Seek>(archive: &ZipArchive<R>) -> Option<String> {
    for name in archive.file_names() {
        if text_utils::ends_with_ascii_ci(name, ".opf") {
            return Some(name.to_string());
        }
    }
    None
}

/// Reads a named entry from a ZIP archive.
fn read_zip_entry<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>> {
    let mut file = archive
        .by_name(name)
        .map_err(|_| EruditioError::Format(format!("Missing ZIP entry: {}", name)))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;
    Ok(data)
}

fn is_content_type(media_type: &str) -> bool {
    media_type.contains("xhtml") || media_type.contains("html")
}

fn is_html_href(href: &str) -> bool {
    text_utils::ends_with_ascii_ci(href, ".html")
        || text_utils::ends_with_ascii_ci(href, ".htm")
        || text_utils::ends_with_ascii_ci(href, ".xhtml")
}

fn guess_media_type(filename: &str) -> &'static str {
    if text_utils::ends_with_ascii_ci(filename, ".jpg")
        || text_utils::ends_with_ascii_ci(filename, ".jpeg")
    {
        "image/jpeg"
    } else if text_utils::ends_with_ascii_ci(filename, ".png") {
        "image/png"
    } else if text_utils::ends_with_ascii_ci(filename, ".gif") {
        "image/gif"
    } else if text_utils::ends_with_ascii_ci(filename, ".svg") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    }
}

fn unescape_xml(s: &str) -> Cow<'_, str> {
    let bytes = s.as_bytes();
    // Fast path: if no ampersand, borrow — zero allocation.
    let first_amp = match memchr::memchr(b'&', bytes) {
        Some(pos) => pos,
        None => return Cow::Borrowed(s),
    };

    let mut result = String::with_capacity(s.len());
    result.push_str(&s[..first_amp]);
    let mut i = first_amp;

    while i < bytes.len() {
        if bytes[i] == b'&' {
            let rest = &s[i..];
            if rest.starts_with("&amp;") {
                result.push('&');
                i += 5;
            } else if rest.starts_with("&lt;") {
                result.push('<');
                i += 4;
            } else if rest.starts_with("&gt;") {
                result.push('>');
                i += 4;
            } else if rest.starts_with("&quot;") {
                result.push('"');
                i += 6;
            } else if rest.starts_with("&apos;") {
                result.push('\'');
                i += 6;
            } else {
                result.push('&');
                i += 1;
            }
        } else {
            // Find next ampersand or end of string
            let next_amp = memchr::memchr(b'&', &bytes[i..])
                .map(|p| i + p)
                .unwrap_or(bytes.len());
            result.push_str(&s[i..next_amp]);
            i = next_amp;
        }
    }
    Cow::Owned(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oeb_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("OEB Test Book".into());
        book.metadata.authors.push("Test Author".into());
        book.metadata.language = Some("en".into());
        book.add_chapter(Chapter {
            title: Some("Chapter One".into()),
            content: "<p>Hello from OEB format!</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Chapter Two".into()),
            content: "<p>Second chapter content.</p>".into(),
            id: Some("ch2".into()),
        });

        // Write.
        let mut output = Vec::new();
        OebWriter::new().write_book(&book, &mut output).unwrap();

        // Read back.
        let mut cursor = Cursor::new(output);
        let decoded = OebReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("OEB Test Book"));
        assert_eq!(decoded.metadata.authors, vec!["Test Author"]);
        assert_eq!(decoded.metadata.language.as_deref(), Some("en"));

        let chapters = decoded.chapters();
        assert_eq!(chapters.len(), 2);
        assert!(chapters[0].content.contains("Hello from OEB format!"));
        assert!(chapters[1].content.contains("Second chapter content."));
    }

    #[test]
    fn oeb_preserves_metadata() {
        let mut book = Book::new();
        book.metadata.title = Some("Meta Book".into());
        book.metadata.authors.push("Alice".into());
        book.metadata.authors.push("Bob".into());
        book.metadata.description = Some("A test description".into());
        book.metadata.publisher = Some("Test Press".into());
        book.metadata.language = Some("fr".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        OebWriter::new().write_book(&book, &mut output).unwrap();

        let mut cursor = Cursor::new(output);
        let decoded = OebReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("Meta Book"));
        assert_eq!(decoded.metadata.authors.len(), 2);
        assert_eq!(
            decoded.metadata.description.as_deref(),
            Some("A test description")
        );
        assert_eq!(decoded.metadata.publisher.as_deref(), Some("Test Press"));
        assert_eq!(decoded.metadata.language.as_deref(), Some("fr"));
    }

    #[test]
    fn parse_opf_manifest_works() {
        let opf = r#"
        <package>
            <manifest>
                <item id="ch1" href="chapter1.xhtml" media-type="application/xhtml+xml" />
                <item id="img1" href="images/cover.jpg" media-type="image/jpeg" />
            </manifest>
        </package>"#;

        let items = parse_opf_manifest(opf);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0, "ch1");
        assert_eq!(items[0].1, "chapter1.xhtml");
        assert_eq!(items[1].0, "img1");
    }

    #[test]
    fn parse_opf_spine_works() {
        let opf = r#"
        <package>
            <spine>
                <itemref idref="ch1" />
                <itemref idref="ch2" />
            </spine>
        </package>"#;

        let idrefs = parse_opf_spine(opf);
        assert_eq!(idrefs, vec!["ch1", "ch2"]);
    }

    #[test]
    fn extract_body_works() {
        let html = "<html><body><p>Hello</p></body></html>";
        assert_eq!(extract_body(html), Some("<p>Hello</p>"));
    }

    #[test]
    fn extract_attr_works() {
        let tag = r#"<item id="ch1" href="file.xhtml" media-type="text/html" />"#;
        assert_eq!(extract_attr(tag, "id"), Some("ch1".into()));
        assert_eq!(extract_attr(tag, "href"), Some("file.xhtml".into()));
        assert_eq!(extract_attr(tag, "media-type"), Some("text/html".into()));
    }

    #[test]
    fn extract_body_case_insensitive() {
        let html = "<HTML><BODY><p>Content</p></BODY></HTML>";
        assert_eq!(extract_body(html), Some("<p>Content</p>"));
    }

    #[test]
    fn extract_body_mixed_case() {
        let html = "<html><Body class=\"main\"><p>Hello</p></body></html>";
        assert_eq!(extract_body(html), Some("<p>Hello</p>"));
    }
}
