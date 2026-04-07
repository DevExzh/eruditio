//! HTMLZ format: HTML inside a ZIP archive.
//!
//! Produces a calibre-compatible HTMLZ archive with:
//! - `index.html` — HTML content with relative image paths
//! - `metadata.opf` — OPF metadata (Dublin Core)
//! - `images/` — extracted image resources

use crate::domain::{Book, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use crate::formats::common::MAX_INPUT_SIZE;
use crate::formats::common::html_utils::{escape_html, strip_leading_heading};
use crate::formats::common::text_utils::ends_with_ascii_ci;
use crate::formats::common::xml_utils;
use crate::formats::html::HtmlReader;
use quick_xml::Reader as XmlReader;
use quick_xml::events::Event;
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
        (&mut *reader)
            .take(MAX_INPUT_SIZE)
            .read_to_end(&mut buffer)?;
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
            if name.starts_with("images/")
                && name.len() > "images/".len()
                && let Ok(mut file) = archive.by_name(name)
            {
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
            zip.start_file("metadata.opf", options).map_err(|e| {
                EruditioError::Format(format!("Failed to write metadata.opf: {}", e))
            })?;
            zip.write_all(opf.as_bytes())?;

            // 3. Write style.css
            zip.start_file("style.css", options)
                .map_err(|e| EruditioError::Format(format!("Failed to write style.css: {}", e)))?;
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
    let chapters = book.chapter_views();

    let mut body = String::with_capacity(4096);
    for (i, chapter) in chapters.iter().enumerate() {
        if i > 0 {
            body.push_str("<hr />\n");
        }
        if let Some(ref ch_title) = chapter.title {
            body.push_str(&format!("<h2>{}</h2>\n", escape_html(ch_title)));
        }
        let content = match chapter.title {
            Some(ref t) => strip_leading_heading(&chapter.content, t),
            None => &chapter.content,
        };
        // Strip XHTML wrapper elements (<?xml>, <!DOCTYPE>, <html>, <head>, <body>)
        // that are present in EPUB chapter content to avoid nested HTML documents.
        // Also strip <link> tags referencing CSS files that don't exist in the
        // HTMLZ archive (the archive has its own style.css linked from the outer doc).
        let content = strip_xhtml_wrapper(content);
        let content = strip_link_tags(&content);
        body.push_str(&content);
        body.push('\n');
    }

    // DO NOT embed resources as data URIs — they are written as separate ZIP entries

    // Rewrite <img> src attributes to add "images/" prefix where needed.
    // Images are stored under images/ in the ZIP, but chapter HTML may reference
    // them without the prefix (e.g. src="cover.jpg" instead of src="images/cover.jpg").
    let body = rewrite_image_src(&body);

    let mut html = crate::formats::html::parser::build_html_document(title, &book.metadata, &body);

    // Inject stylesheet link into <head> (before </head>)
    if let Some(pos) = html.find("</head>") {
        html.insert_str(pos, "<link rel=\"stylesheet\" href=\"style.css\">\n");
    }

    html
}

/// Rewrites `<img` tag `src` attributes to prepend `images/` where needed.
///
/// Any `src` value that does not already start with `images/`, `http://`,
/// `https://`, or `data:` gets the `images/` prefix prepended.
fn rewrite_image_src(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut remaining = html;

    while let Some(img_pos) = remaining.find("<img") {
        // Copy everything up to and including `<img`
        result.push_str(&remaining[..img_pos + 4]);
        remaining = &remaining[img_pos + 4..];

        // Look for src= within this tag (before the closing `>`)
        if let Some(tag_end) = remaining.find('>') {
            let tag_content = &remaining[..=tag_end];
            if let Some(src_offset) = tag_content.find("src=\"") {
                let after_src_start = &remaining[src_offset + 5..];
                if let Some(quote_end) = after_src_start.find('"') {
                    let src_value = &after_src_start[..quote_end];
                    if !src_value.starts_with("images/")
                        && !src_value.starts_with("http://")
                        && !src_value.starts_with("https://")
                        && !src_value.starts_with("data:")
                    {
                        result.push_str(&remaining[..src_offset + 5]);
                        result.push_str("images/");
                        result.push_str(src_value);
                        result.push('"');
                        remaining = &after_src_start[quote_end + 1..];
                    } else {
                        // src already has a valid prefix, copy the tag as-is
                        result.push_str(&remaining[..=tag_end]);
                        remaining = &remaining[tag_end + 1..];
                    }
                } else {
                    result.push_str(&remaining[..=tag_end]);
                    remaining = &remaining[tag_end + 1..];
                }
            } else {
                // No src= in this img tag, copy the tag as-is
                result.push_str(&remaining[..=tag_end]);
                remaining = &remaining[tag_end + 1..];
            }
        }
        // else: no closing `>` found — just continue (broken HTML, copy rest at end)
    }

    // Copy any remaining content after the last <img> tag
    result.push_str(remaining);
    result
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
    for (i, author) in m.authors.iter().enumerate() {
        if i == 0 {
            if let Some(ref sort) = m.author_sort {
                xml.push_str("    <dc:creator opf:file-as=\"");
                xml.push_str(&escape_html(sort));
                xml.push_str("\" opf:role=\"aut\">");
            } else {
                xml.push_str("    <dc:creator opf:role=\"aut\">");
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
    if let Some(ref date) = m.publication_date {
        xml.push_str(&format!(
            "    <dc:date>{}</dc:date>\n",
            date.format("%Y-%m-%d")
        ));
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
    if let Some(ref cover_id) = m.cover_image_id {
        xml.push_str("    <meta name=\"cover\" content=\"");
        xml.push_str(&escape_html(cover_id));
        xml.push_str("\"/>\n");
    }

    xml.push_str("  </metadata>\n");

    // Guide section (cover and other reference types)
    if !book.guide.is_empty() {
        xml.push_str("  <guide>\n");
        for r in &book.guide.references {
            xml.push_str("    <reference type=\"");
            xml.push_str(&escape_html(r.ref_type.as_str()));
            xml.push_str("\" title=\"");
            xml.push_str(&escape_html(&r.title));
            xml.push_str("\" href=\"");
            xml.push_str("index.html");
            xml.push_str("\"/>\n");
        }
        xml.push_str("  </guide>\n");
    }

    xml.push_str("</package>\n");
    xml
}

/// Finds the first HTML file in a ZIP archive.
fn find_html_file<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Option<String> {
    for i in 0..archive.len() {
        if let Ok(file) = archive.by_index(i) {
            let name = file.name();
            if ends_with_ascii_ci(name, ".html")
                || ends_with_ascii_ci(name, ".htm")
                || ends_with_ascii_ci(name, ".xhtml")
            {
                return Some(name.to_string());
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
    let mut current_file_as: Option<String> = None;
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

                    // Track the opf:file-as attribute on <dc:creator>
                    if tag == "creator" {
                        current_file_as = xml_utils::get_attribute(e, "opf:file-as")
                            .or_else(|| xml_utils::get_attribute(e, "file-as"));
                    }
                }
            },
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
                            },
                            "calibre:series_index" => {
                                if book.metadata.series_index.is_none()
                                    && let Ok(idx) = content.parse::<f64>()
                                {
                                    book.metadata.series_index = Some(idx);
                                }
                            },
                            "cover" => {
                                if book.metadata.cover_image_id.is_none() {
                                    book.metadata.cover_image_id = Some(content);
                                }
                            },
                            _ => {},
                        }
                    }
                }
            },
            Ok(Event::Text(ref e)) => {
                if in_metadata && !current_tag.is_empty() {
                    current_text = xml_utils::bytes_to_string(e.as_ref());
                }
            },
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
                            },
                            "creator" => {
                                // Capture opf:file-as from the first creator
                                if book.metadata.author_sort.is_none()
                                    && let Some(ref fa) = current_file_as
                                {
                                    book.metadata.author_sort = Some(fa.clone());
                                }
                                opf_authors.push(text);
                            },
                            "language" => {
                                if book.metadata.language.is_none() {
                                    book.metadata.language = Some(text);
                                }
                            },
                            "publisher" => {
                                if book.metadata.publisher.is_none() {
                                    book.metadata.publisher = Some(text);
                                }
                            },
                            "identifier" => {
                                if let Some(ref scheme) = current_scheme {
                                    if scheme.eq_ignore_ascii_case("ISBN")
                                        && book.metadata.isbn.is_none()
                                    {
                                        book.metadata.isbn = Some(text);
                                    }
                                } else if book.metadata.identifier.is_none() {
                                    book.metadata.identifier = Some(text);
                                }
                            },
                            "description" => {
                                if book.metadata.description.is_none() {
                                    book.metadata.description = Some(text);
                                }
                            },
                            "subject" => {
                                if !book.metadata.subjects.contains(&text) {
                                    book.metadata.subjects.push(text);
                                }
                            },
                            "rights" => {
                                if book.metadata.rights.is_none() {
                                    book.metadata.rights = Some(text);
                                }
                            },
                            "date" => {
                                if book.metadata.publication_date.is_none() {
                                    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&text) {
                                        book.metadata.publication_date =
                                            Some(dt.with_timezone(&chrono::Utc));
                                    } else if let Ok(date) =
                                        chrono::NaiveDate::parse_from_str(&text, "%Y-%m-%d")
                                    {
                                        book.metadata.publication_date =
                                            date.and_hms_opt(0, 0, 0).and_then(|ndt| {
                                                ndt.and_local_timezone(chrono::Utc).single()
                                            });
                                    } else if let Ok(year) = text.parse::<i32>() {
                                        book.metadata.publication_date =
                                            chrono::NaiveDate::from_ymd_opt(year, 1, 1)
                                                .and_then(|d| d.and_hms_opt(0, 0, 0))
                                                .and_then(|ndt| {
                                                    ndt.and_local_timezone(chrono::Utc).single()
                                                });
                                    }
                                }
                            },
                            _ => {},
                        }
                    }
                    current_tag.clear();
                    current_text.clear();
                }
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {},
        }
        buf.clear();
    }

    // Authors from OPF take precedence if the HTML-parsed ones are empty.
    if book.metadata.authors.is_empty() && !opf_authors.is_empty() {
        book.metadata.authors = opf_authors;
    }
}

/// Strips XHTML wrapper elements from chapter content so that only the inner
/// body content remains. This removes XML processing instructions, DOCTYPE
/// declarations, `<html>`, `<head>` (with contents), and `<body>` tags that
/// are present in EPUB XHTML source files.
///
/// Similar to the `strip_xhtml_wrapper` in the MOBI writer, adapted for HTMLZ.
fn strip_xhtml_wrapper(content: &str) -> String {
    let mut s = content.to_string();

    // Remove <?xml ...?> processing instructions.
    while let Some(start) = s.find("<?xml") {
        if let Some(end) = s[start..].find("?>") {
            s.replace_range(start..start + end + 2, "");
        } else {
            break;
        }
    }

    // Remove <!DOCTYPE ...> declarations.
    while let Some(start) = s.find("<!DOCTYPE") {
        if let Some(end) = s[start..].find('>') {
            s.replace_range(start..start + end + 1, "");
        } else {
            break;
        }
    }
    // Also handle lowercase variant.
    while let Some(start) = s.find("<!doctype") {
        if let Some(end) = s[start..].find('>') {
            s.replace_range(start..start + end + 1, "");
        } else {
            break;
        }
    }

    // Remove <head>...</head> blocks (including contents like <style>, <link>, <meta>, <title>).
    // Must verify the character after "<head" is '>' or whitespace to avoid matching <header>.
    {
        let mut search_from = 0;
        while let Some(rel) = s[search_from..].find("<head") {
            let start = search_from + rel;
            let after = start + 5; // length of "<head"
            let next_ch = s.as_bytes().get(after).copied().unwrap_or(b'>');
            if next_ch != b'>' && !next_ch.is_ascii_whitespace() && next_ch != b'/' {
                // Matched something like <header> — skip it.
                search_from = start + 5;
                continue;
            }
            if let Some(end) = s[start..].find("</head>") {
                s.replace_range(start..start + end + 7, "");
                search_from = start; // re-check from same position
            } else {
                break;
            }
        }
    }
    // Also handle uppercase <HEAD>...</HEAD>.
    {
        let mut search_from = 0;
        while let Some(rel) = s[search_from..].find("<HEAD") {
            let start = search_from + rel;
            let after = start + 5;
            let next_ch = s.as_bytes().get(after).copied().unwrap_or(b'>');
            if next_ch != b'>' && !next_ch.is_ascii_whitespace() && next_ch != b'/' {
                search_from = start + 5;
                continue;
            }
            if let Some(end) = s[start..].find("</HEAD>") {
                s.replace_range(start..start + end + 7, "");
                search_from = start;
            } else {
                break;
            }
        }
    }

    // Remove <html ...> and </html> tags.
    while let Some(start) = s.find("<html") {
        if let Some(end) = s[start..].find('>') {
            s.replace_range(start..start + end + 1, "");
        } else {
            break;
        }
    }
    while let Some(start) = s.find("<HTML") {
        if let Some(end) = s[start..].find('>') {
            s.replace_range(start..start + end + 1, "");
        } else {
            break;
        }
    }
    while let Some(start) = s.find("</html>") {
        s.replace_range(start..start + 7, "");
    }
    while let Some(start) = s.find("</HTML>") {
        s.replace_range(start..start + 7, "");
    }

    // Remove <body ...> and </body> tags.
    while let Some(start) = s.find("<body") {
        if let Some(end) = s[start..].find('>') {
            s.replace_range(start..start + end + 1, "");
        } else {
            break;
        }
    }
    while let Some(start) = s.find("<BODY") {
        if let Some(end) = s[start..].find('>') {
            s.replace_range(start..start + end + 1, "");
        } else {
            break;
        }
    }
    while let Some(start) = s.find("</body>") {
        s.replace_range(start..start + 7, "");
    }
    while let Some(start) = s.find("</BODY>") {
        s.replace_range(start..start + 7, "");
    }

    s
}

/// Strips `<link ...>` tags from HTML content.
///
/// These are self-closing tags that reference external CSS files (e.g.
/// `<link rel="stylesheet" href="pgepub.css">`). In the original EPUB
/// chapters they reference CSS files that don't exist in the HTMLZ archive.
/// The HTMLZ archive has its own `style.css` linked from the outer document.
fn strip_link_tags(content: &str) -> String {
    let mut s = content.to_string();

    // Search for "<link" followed by whitespace or ">", to avoid false matches.
    {
        let mut search_from = 0;
        while let Some(rel) = s[search_from..].find("<link") {
            let start = search_from + rel;
            let after = start + 5; // length of "<link"
            let next_ch = s.as_bytes().get(after).copied().unwrap_or(b'>');
            if !next_ch.is_ascii_whitespace() && next_ch != b'>' && next_ch != b'/' {
                search_from = start + 5;
                continue;
            }
            if let Some(end_offset) = s[start..].find('>') {
                s.replace_range(start..start + end_offset + 1, "");
                search_from = start;
            } else {
                break;
            }
        }
    }
    // Also handle <LINK ...> (uppercase).
    {
        let mut search_from = 0;
        while let Some(rel) = s[search_from..].find("<LINK") {
            let start = search_from + rel;
            let after = start + 5;
            let next_ch = s.as_bytes().get(after).copied().unwrap_or(b'>');
            if !next_ch.is_ascii_whitespace() && next_ch != b'>' && next_ch != b'/' {
                search_from = start + 5;
                continue;
            }
            if let Some(end_offset) = s[start..].find('>') {
                s.replace_range(start..start + end_offset + 1, "");
                search_from = start;
            } else {
                break;
            }
        }
    }

    s
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
    match filename
        .rsplit('.')
        .next()
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
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
        book.add_chapter(Chapter {
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
        book.add_chapter(Chapter {
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
        book.add_chapter(Chapter {
            title: None,
            content: "<p>Hi</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        // Verify the ZIP contains metadata.opf
        let cursor = Cursor::new(output);
        let mut archive = ZipArchive::new(cursor).unwrap();
        let mut opf = archive
            .by_name("metadata.opf")
            .expect("metadata.opf should exist");
        let mut opf_content = String::new();
        opf.read_to_string(&mut opf_content).unwrap();

        assert!(opf_content.contains("<dc:title>OPF Test</dc:title>"));
        assert!(opf_content.contains("opf:role=\"aut\">Bob</dc:creator>"));
    }

    #[test]
    fn htmlz_writer_includes_images() {
        let mut book = Book::new();
        book.metadata.title = Some("Image Test".into());
        book.add_chapter(Chapter {
            title: None,
            content: "<p>text</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_resource(
            "img1",
            "images/cover.png",
            vec![0x89, 0x50, 0x4E, 0x47],
            "image/png",
        );

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
        book.add_chapter(Chapter {
            title: None,
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        let opf = generate_htmlz_opf(&book);

        assert!(opf.contains("<dc:title>Full Meta</dc:title>"));
        assert!(opf.contains("<dc:creator opf:role=\"aut\">Author A</dc:creator>"));
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
        book.add_chapter(Chapter {
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
        book.add_chapter(Chapter {
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
        book.add_chapter(Chapter {
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
        book.add_chapter(Chapter {
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
        let mut css_file = archive
            .by_name("style.css")
            .expect("style.css should exist");
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
        book.add_chapter(Chapter {
            title: None,
            content: "<p>Plain content</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        // Verify the ZIP contains style.css with default stylesheet
        let cursor = Cursor::new(output);
        let mut archive = ZipArchive::new(cursor).unwrap();
        let mut css_file = archive
            .by_name("style.css")
            .expect("style.css should exist");
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
        book.add_chapter(Chapter {
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
        book.add_chapter(Chapter {
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

    #[test]
    fn htmlz_opf_writes_date_author_sort_cover_and_guide() {
        use crate::domain::guide::{Guide, GuideReference, GuideType};
        use chrono::TimeZone;

        let mut book = Book::new();
        book.metadata.title = Some("Extended Meta".into());
        book.metadata.authors.push("Alice Author".into());
        book.metadata.authors.push("Bob Writer".into());
        book.metadata.author_sort = Some("Author, Alice".into());
        book.metadata.publication_date =
            Some(chrono::Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap());
        book.metadata.cover_image_id = Some("cover-img".into());
        book.guide = Guide {
            references: vec![
                GuideReference {
                    ref_type: GuideType::Cover,
                    title: "Cover".into(),
                    href: "cover.xhtml".into(),
                },
                GuideReference {
                    ref_type: GuideType::Toc,
                    title: "Table of Contents".into(),
                    href: "toc.xhtml".into(),
                },
            ],
        };
        book.add_chapter(Chapter {
            title: None,
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        let opf = generate_htmlz_opf(&book);

        // Verify date
        assert!(
            opf.contains("<dc:date>2024-06-15</dc:date>"),
            "OPF should contain publication date. Got:\n{}",
            opf
        );

        // Verify file-as on first author only
        assert!(
            opf.contains(
                "opf:file-as=\"Author, Alice\" opf:role=\"aut\">Alice Author</dc:creator>"
            ),
            "First author should have opf:file-as attribute. Got:\n{}",
            opf
        );
        // Second author should NOT have file-as
        assert!(
            opf.contains("<dc:creator>Bob Writer</dc:creator>"),
            "Second author should not have file-as. Got:\n{}",
            opf
        );

        // Verify cover meta
        assert!(
            opf.contains("<meta name=\"cover\" content=\"cover-img\"/>"),
            "OPF should contain cover meta. Got:\n{}",
            opf
        );

        // Verify guide section
        assert!(
            opf.contains("<guide>"),
            "OPF should contain guide section. Got:\n{}",
            opf
        );
        assert!(
            opf.contains("type=\"cover\""),
            "Guide should contain cover reference. Got:\n{}",
            opf
        );
        assert!(
            opf.contains("title=\"Cover\""),
            "Guide cover should have title. Got:\n{}",
            opf
        );
        assert!(
            opf.contains("href=\"index.html\""),
            "Guide cover should have href pointing to index.html. Got:\n{}",
            opf
        );
        assert!(
            opf.contains("type=\"toc\""),
            "Guide should contain toc reference. Got:\n{}",
            opf
        );
        assert!(
            opf.contains("</guide>"),
            "Guide section should be closed. Got:\n{}",
            opf
        );
    }

    #[test]
    fn merge_opf_parses_date_file_as_and_cover() {
        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>Date Test</dc:title>
    <dc:creator opf:file-as="Doe, Jane">Jane Doe</dc:creator>
    <dc:date>2024-06-15</dc:date>
    <meta name="cover" content="cover-img"/>
  </metadata>
</package>"#;

        let mut book = Book::new();
        merge_opf_metadata(opf, &mut book);

        assert_eq!(book.metadata.title.as_deref(), Some("Date Test"));
        assert_eq!(book.metadata.authors, vec!["Jane Doe"]);
        assert_eq!(
            book.metadata.author_sort.as_deref(),
            Some("Doe, Jane"),
            "author_sort should be parsed from opf:file-as"
        );
        assert!(
            book.metadata.publication_date.is_some(),
            "publication_date should be parsed from dc:date"
        );
        let date = book.metadata.publication_date.unwrap();
        assert_eq!(date.format("%Y-%m-%d").to_string(), "2024-06-15");
        assert_eq!(
            book.metadata.cover_image_id.as_deref(),
            Some("cover-img"),
            "cover_image_id should be parsed from meta cover"
        );
    }

    #[test]
    fn htmlz_round_trip_extended_metadata() {
        use crate::domain::guide::{Guide, GuideReference, GuideType};
        use chrono::TimeZone;

        let mut book = Book::new();
        book.metadata.title = Some("Round Trip Extended".into());
        book.metadata.authors.push("Jane Doe".into());
        book.metadata.author_sort = Some("Doe, Jane".into());
        book.metadata.publication_date =
            Some(chrono::Utc.with_ymd_and_hms(2024, 3, 20, 0, 0, 0).unwrap());
        book.metadata.cover_image_id = Some("cover-img".into());
        book.guide = Guide {
            references: vec![GuideReference {
                ref_type: GuideType::Cover,
                title: "Cover".into(),
                href: "cover.xhtml".into(),
            }],
        };
        book.add_chapter(Chapter {
            title: None,
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        // Write
        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        // Read back
        let mut cursor = Cursor::new(output);
        let decoded = HtmlzReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(
            decoded.metadata.author_sort.as_deref(),
            Some("Doe, Jane"),
            "author_sort should round-trip"
        );
        assert!(
            decoded.metadata.publication_date.is_some(),
            "publication_date should round-trip"
        );
        let date = decoded.metadata.publication_date.unwrap();
        assert_eq!(date.format("%Y-%m-%d").to_string(), "2024-03-20");
        assert_eq!(
            decoded.metadata.cover_image_id.as_deref(),
            Some("cover-img"),
            "cover_image_id should round-trip"
        );
    }

    #[test]
    fn htmlz_writer_preserves_text_css_from_epub_reader() {
        // Simulates the EPUB reader path: CSS is loaded as ManifestData::Text
        // (not ManifestData::Inline) because text/css is a text media type.
        // The HTMLZ writer must still include this CSS in the output.
        use crate::domain::ManifestItem;

        let mut book = Book::new();
        book.metadata.title = Some("CSS Passthrough".into());
        book.add_chapter(Chapter {
            title: None,
            content: "<p>Styled content</p>".into(),
            id: Some("ch1".into()),
        });

        // Insert CSS as ManifestData::Text, the way the EPUB reader does it.
        let css_item = ManifestItem::new("epub-style", "styles/epub.css", "text/css")
            .with_text("body { margin: 2em; }\nh1 { color: navy; }");
        book.manifest.insert(css_item);

        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        // Verify the ZIP contains style.css with the EPUB CSS content
        let cursor = Cursor::new(output);
        let mut archive = ZipArchive::new(cursor).unwrap();
        let mut css_file = archive
            .by_name("style.css")
            .expect("style.css should exist");
        let mut css_content = String::new();
        css_file.read_to_string(&mut css_content).unwrap();
        assert!(
            css_content.contains("margin: 2em"),
            "style.css should contain EPUB CSS, not just the default. Got: {}",
            css_content
        );
        assert!(
            css_content.contains("color: navy"),
            "style.css should contain all EPUB CSS rules. Got: {}",
            css_content
        );
        // It should NOT fall back to the default stylesheet
        assert!(
            !css_content.contains("font-family: serif"),
            "style.css should use EPUB CSS, not the default stylesheet. Got: {}",
            css_content
        );
    }

    #[test]
    fn htmlz_writer_concatenates_multiple_text_css() {
        // When an EPUB has multiple CSS files (all loaded as ManifestData::Text),
        // the HTMLZ writer should concatenate them all into style.css.
        use crate::domain::ManifestItem;

        let mut book = Book::new();
        book.metadata.title = Some("Multi CSS".into());
        book.add_chapter(Chapter {
            title: None,
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        let css1 = ManifestItem::new("css1", "styles/reset.css", "text/css")
            .with_text("* { margin: 0; padding: 0; }");
        let css2 = ManifestItem::new("css2", "styles/main.css", "text/css")
            .with_text("body { font-size: 16px; }");
        book.manifest.insert(css1);
        book.manifest.insert(css2);

        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        let cursor = Cursor::new(output);
        let mut archive = ZipArchive::new(cursor).unwrap();
        let mut css_file = archive.by_name("style.css").unwrap();
        let mut css_content = String::new();
        css_file.read_to_string(&mut css_content).unwrap();

        assert!(
            css_content.contains("margin: 0") && css_content.contains("font-size: 16px"),
            "style.css should contain CSS from both files. Got: {}",
            css_content
        );
    }

    #[test]
    fn rewrite_image_src_adds_images_prefix() {
        let input = r#"<p>Text</p><img src="cover.jpg"><img src="photo.png">"#;
        let result = rewrite_image_src(input);
        assert!(
            result.contains(r#"src="images/cover.jpg""#),
            "Should prepend images/ to cover.jpg. Got: {}",
            result
        );
        assert!(
            result.contains(r#"src="images/photo.png""#),
            "Should prepend images/ to photo.png. Got: {}",
            result
        );
    }

    #[test]
    fn rewrite_image_src_preserves_already_prefixed() {
        let input = r#"<img src="images/cover.jpg"><img src="http://example.com/img.png"><img src="https://example.com/img.png"><img src="data:image/png;base64,abc">"#;
        let result = rewrite_image_src(input);
        assert!(
            result.contains(r#"src="images/cover.jpg""#),
            "Should not double-prefix images/. Got: {}",
            result
        );
        assert!(
            !result.contains(r#"src="images/images/"#),
            "Should not double-prefix images/. Got: {}",
            result
        );
        assert!(
            result.contains(r#"src="http://example.com/img.png""#),
            "Should preserve http:// URLs. Got: {}",
            result
        );
        assert!(
            result.contains(r#"src="https://example.com/img.png""#),
            "Should preserve https:// URLs. Got: {}",
            result
        );
        assert!(
            result.contains(r#"src="data:image/png;base64,abc""#),
            "Should preserve data: URIs. Got: {}",
            result
        );
    }

    #[test]
    fn rewrite_image_src_no_img_tags() {
        let input = "<p>No images here</p>";
        let result = rewrite_image_src(input);
        assert_eq!(result, input);
    }

    #[test]
    fn htmlz_content_uses_h2_for_chapter_titles() {
        let mut book = Book::new();
        book.metadata.title = Some("Heading Test".into());
        book.add_chapter(Chapter {
            title: Some("Chapter One".into()),
            content: "<p>First chapter content</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Chapter Two".into()),
            content: "<p>Second chapter content</p>".into(),
            id: Some("ch2".into()),
        });

        let html = generate_htmlz_content(&book);

        assert!(
            html.contains("<h2>Chapter One</h2>"),
            "Chapter titles should be h2, not h1. Got: {}",
            html
        );
        assert!(
            html.contains("<h2>Chapter Two</h2>"),
            "Chapter titles should be h2, not h1. Got: {}",
            html
        );
        // There should be no h1 tags for chapter titles
        assert!(
            !html.contains("<h1>Chapter One</h1>"),
            "Should not use h1 for chapter titles. Got: {}",
            html
        );
    }

    #[test]
    fn htmlz_content_rewrites_image_src_paths() {
        let mut book = Book::new();
        book.metadata.title = Some("Image Path Test".into());
        book.add_chapter(Chapter {
            title: None,
            content: r#"<p>Before</p><img src="cover.jpg"><p>After</p>"#.into(),
            id: Some("ch1".into()),
        });

        let html = generate_htmlz_content(&book);

        assert!(
            html.contains(r#"src="images/cover.jpg""#),
            "Image src should be rewritten to images/cover.jpg. Got: {}",
            html
        );
    }

    #[test]
    fn htmlz_opf_guide_refs_point_to_index_html() {
        use crate::domain::guide::{Guide, GuideReference, GuideType};

        let mut book = Book::new();
        book.metadata.title = Some("Guide Test".into());
        book.guide = Guide {
            references: vec![
                GuideReference {
                    ref_type: GuideType::Cover,
                    title: "Cover".into(),
                    href: "wrap0000.html".into(),
                },
                GuideReference {
                    ref_type: GuideType::Toc,
                    title: "Table of Contents".into(),
                    href: "11-h-0.htm.html".into(),
                },
            ],
        };
        book.add_chapter(Chapter {
            title: None,
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        let opf = generate_htmlz_opf(&book);

        // All guide hrefs should point to index.html
        assert!(
            !opf.contains("wrap0000.html"),
            "Guide should not reference original EPUB filenames. Got:\n{}",
            opf
        );
        assert!(
            !opf.contains("11-h-0.htm.html"),
            "Guide should not reference original EPUB filenames. Got:\n{}",
            opf
        );
        // Count occurrences of index.html in guide section
        assert!(
            opf.contains("href=\"index.html\""),
            "Guide references should point to index.html. Got:\n{}",
            opf
        );
    }

    #[test]
    fn htmlz_opf_first_author_has_role_aut() {
        let mut book = Book::new();
        book.metadata.title = Some("Role Test".into());
        book.metadata.authors.push("First Author".into());
        book.metadata.authors.push("Second Author".into());
        book.add_chapter(Chapter {
            title: None,
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        let opf = generate_htmlz_opf(&book);

        assert!(
            opf.contains("opf:role=\"aut\">First Author</dc:creator>"),
            "First author should have opf:role=\"aut\". Got:\n{}",
            opf
        );
        // Second author should NOT have role
        assert!(
            opf.contains("<dc:creator>Second Author</dc:creator>"),
            "Second author should not have opf:role. Got:\n{}",
            opf
        );
    }

    #[test]
    fn htmlz_opf_first_author_role_with_file_as() {
        let mut book = Book::new();
        book.metadata.title = Some("Role+Sort Test".into());
        book.metadata.authors.push("Jane Doe".into());
        book.metadata.author_sort = Some("Doe, Jane".into());
        book.add_chapter(Chapter {
            title: None,
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        let opf = generate_htmlz_opf(&book);

        assert!(
            opf.contains("opf:file-as=\"Doe, Jane\" opf:role=\"aut\">Jane Doe</dc:creator>"),
            "First author should have both opf:file-as and opf:role. Got:\n{}",
            opf
        );
    }

    // -- strip_xhtml_wrapper tests ------------------------------------------

    #[test]
    fn strip_xhtml_wrapper_removes_xml_declaration() {
        let input = r#"<?xml version="1.0" encoding="UTF-8"?><p>Content</p>"#;
        let result = strip_xhtml_wrapper(input);
        assert!(
            !result.contains("<?xml"),
            "Should remove XML declaration. Got: {}",
            result
        );
        assert!(result.contains("<p>Content</p>"));
    }

    #[test]
    fn strip_xhtml_wrapper_removes_doctype() {
        let input = "<!DOCTYPE html><p>Content</p>";
        let result = strip_xhtml_wrapper(input);
        assert!(
            !result.contains("<!DOCTYPE"),
            "Should remove DOCTYPE. Got: {}",
            result
        );
        assert!(result.contains("<p>Content</p>"));
    }

    #[test]
    fn strip_xhtml_wrapper_removes_full_xhtml_doc() {
        let input = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd">
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
  <title>Chapter 1</title>
  <link rel="stylesheet" href="pgepub.css" type="text/css"/>
  <link rel="stylesheet" href="0.css" type="text/css"/>
  <meta http-equiv="Content-Type" content="text/html; charset=utf-8"/>
</head>
<body>
<h1>Chapter 1</h1>
<p>Alice was beginning to get very tired.</p>
</body>
</html>"#;
        let result = strip_xhtml_wrapper(input);
        assert!(
            !result.contains("<?xml"),
            "Should remove XML declaration. Got: {}",
            result
        );
        assert!(
            !result.contains("<!DOCTYPE"),
            "Should remove DOCTYPE. Got: {}",
            result
        );
        assert!(
            !result.contains("<html"),
            "Should remove <html> tag. Got: {}",
            result
        );
        assert!(
            !result.contains("</html>"),
            "Should remove </html> tag. Got: {}",
            result
        );
        assert!(
            !result.contains("<head"),
            "Should remove <head> block. Got: {}",
            result
        );
        assert!(
            !result.contains("</head>"),
            "Should remove </head> tag. Got: {}",
            result
        );
        assert!(
            !result.contains("<title>"),
            "Should remove <title> within head. Got: {}",
            result
        );
        assert!(
            !result.contains("<body"),
            "Should remove <body> tag. Got: {}",
            result
        );
        assert!(
            !result.contains("</body>"),
            "Should remove </body> tag. Got: {}",
            result
        );
        assert!(
            result.contains("<h1>Chapter 1</h1>"),
            "Should preserve body content. Got: {}",
            result
        );
        assert!(
            result.contains("Alice was beginning to get very tired."),
            "Should preserve paragraph content. Got: {}",
            result
        );
    }

    #[test]
    fn strip_xhtml_wrapper_preserves_plain_html() {
        let input = "<p>Just a paragraph</p><p>Another one</p>";
        let result = strip_xhtml_wrapper(input);
        assert_eq!(
            result, input,
            "Plain HTML without wrappers should be unchanged"
        );
    }

    // -- strip_link_tags tests ----------------------------------------------

    #[test]
    fn strip_link_tags_removes_stylesheet_links() {
        let input = r#"<link rel="stylesheet" href="pgepub.css" type="text/css"/><p>Content</p><link rel="stylesheet" href="0.css" type="text/css"/>"#;
        let result = strip_link_tags(input);
        assert!(
            !result.contains("<link"),
            "Should remove all <link> tags. Got: {}",
            result
        );
        assert!(
            result.contains("<p>Content</p>"),
            "Should preserve other content. Got: {}",
            result
        );
    }

    #[test]
    fn strip_link_tags_preserves_content_without_links() {
        let input = "<p>No links here</p>";
        let result = strip_link_tags(input);
        assert_eq!(result, input);
    }

    // -- Integration: no nested HTML documents in HTMLZ output ---------------

    #[test]
    fn htmlz_content_no_nested_html_documents() {
        // Simulate EPUB chapters that contain full XHTML documents
        let mut book = Book::new();
        book.metadata.title = Some("Nested Test".into());
        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd">
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
  <title>Chapter 1</title>
  <link rel="stylesheet" href="pgepub.css" type="text/css"/>
</head>
<body>
<h1>Chapter 1</h1>
<p>First chapter content.</p>
</body>
</html>"#
                .into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Chapter 2".into()),
            content: r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
  <title>Chapter 2</title>
  <link rel="stylesheet" href="0.css" type="text/css"/>
  <link rel="stylesheet" href="pgepub.css" type="text/css"/>
</head>
<body>
<h2>Chapter 2</h2>
<p>Second chapter content.</p>
</body>
</html>"#
                .into(),
            id: Some("ch2".into()),
        });

        let html = generate_htmlz_content(&book);

        // The output HTML document itself has one <html>, <head>, and <body>.
        // Count occurrences: there should be exactly 1 of each opening tag.
        let html_open_count = html.matches("<html").count();
        let body_open_count = html.matches("<body").count();
        let head_open_count = html.matches("<head").count();
        assert_eq!(
            html_open_count, 1,
            "Should have exactly 1 <html> tag (the outer doc). Got {}: {}",
            html_open_count, html
        );
        assert_eq!(
            body_open_count, 1,
            "Should have exactly 1 <body> tag (the outer doc). Got {}: {}",
            body_open_count, html
        );
        assert_eq!(
            head_open_count, 1,
            "Should have exactly 1 <head> block (the outer doc). Got {}: {}",
            head_open_count, html
        );

        // No XML declarations or DOCTYPEs from chapters should remain
        // (the outer doc's DOCTYPE is fine)
        let xml_decl_count = html.matches("<?xml").count();
        assert_eq!(
            xml_decl_count, 0,
            "Should have no <?xml?> declarations. Got {}: {}",
            xml_decl_count, html
        );

        // Chapter content should be preserved
        assert!(
            html.contains("First chapter content."),
            "Chapter 1 content should be preserved. Got: {}",
            html
        );
        assert!(
            html.contains("Second chapter content."),
            "Chapter 2 content should be preserved. Got: {}",
            html
        );
    }

    #[test]
    fn htmlz_content_no_broken_link_references() {
        // Chapters with <link> tags to CSS files that don't exist in HTMLZ
        let mut book = Book::new();
        book.metadata.title = Some("Link Test".into());
        book.add_chapter(Chapter {
            title: None,
            content: r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
  <link rel="stylesheet" href="pgepub.css" type="text/css"/>
  <link rel="stylesheet" href="0.css" type="text/css"/>
</head>
<body>
<p>Content with broken CSS refs.</p>
</body>
</html>"#
                .into(),
            id: Some("ch1".into()),
        });

        let html = generate_htmlz_content(&book);

        // The only <link> should be the outer doc's link to style.css
        assert!(
            !html.contains("pgepub.css"),
            "Should not contain reference to pgepub.css. Got: {}",
            html
        );
        assert!(
            !html.contains("0.css"),
            "Should not contain reference to 0.css. Got: {}",
            html
        );
        // The outer doc's stylesheet link should be present
        assert!(
            html.contains(r#"href="style.css""#),
            "Should contain link to style.css. Got: {}",
            html
        );
        // Content should be preserved
        assert!(
            html.contains("Content with broken CSS refs."),
            "Body content should be preserved. Got: {}",
            html
        );
    }

    #[test]
    fn htmlz_content_preserves_body_content_from_xhtml_chapters() {
        let mut book = Book::new();
        book.metadata.title = Some("Preserve Test".into());
        book.add_chapter(Chapter {
            title: None,
            content: r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Test</title></head>
<body>
<div class="chapter">
  <h2>Introduction</h2>
  <p>This is a <em>test</em> with <strong>formatting</strong>.</p>
  <img src="figure1.png" alt="Figure 1"/>
  <blockquote><p>A famous quote.</p></blockquote>
</div>
</body>
</html>"#
                .into(),
            id: Some("ch1".into()),
        });

        let html = generate_htmlz_content(&book);

        assert!(html.contains("<div class=\"chapter\">"));
        assert!(html.contains("<em>test</em>"));
        assert!(html.contains("<strong>formatting</strong>"));
        assert!(html.contains("alt=\"Figure 1\""));
        assert!(html.contains("<blockquote><p>A famous quote.</p></blockquote>"));
    }

    #[test]
    fn htmlz_full_zip_no_nested_documents() {
        // End-to-end test: write HTMLZ and verify the index.html in the ZIP
        let mut book = Book::new();
        book.metadata.title = Some("E2E Nested Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch 1".into()),
            content: r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
  <title>Ch 1</title>
  <link rel="stylesheet" href="stylesheet.css"/>
  <style>body { margin: 0; }</style>
</head>
<body>
<h1>Ch 1</h1>
<p>Chapter one body.</p>
</body>
</html>"#
                .into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        HtmlzWriter::new().write_book(&book, &mut output).unwrap();

        // Read back the index.html from the ZIP
        let cursor = Cursor::new(output);
        let mut archive = ZipArchive::new(cursor).unwrap();
        let mut html_file = archive.by_name("index.html").unwrap();
        let mut html_content = String::new();
        html_file.read_to_string(&mut html_content).unwrap();

        // Exactly one <html>, <body>, <head>
        assert_eq!(
            html_content.matches("<html").count(),
            1,
            "ZIP index.html should have exactly 1 <html>. Got: {}",
            html_content
        );
        assert_eq!(
            html_content.matches("<body").count(),
            1,
            "ZIP index.html should have exactly 1 <body>. Got: {}",
            html_content
        );
        assert_eq!(
            html_content.matches("<head").count(),
            1,
            "ZIP index.html should have exactly 1 <head>. Got: {}",
            html_content
        );

        // No broken CSS references
        assert!(
            !html_content.contains("stylesheet.css"),
            "Should not contain reference to original stylesheet.css. Got: {}",
            html_content
        );

        // Content preserved
        assert!(
            html_content.contains("Chapter one body."),
            "Chapter content should be preserved. Got: {}",
            html_content
        );

        // Has the proper style.css link
        assert!(
            html_content.contains(r#"href="style.css""#),
            "Should link to style.css. Got: {}",
            html_content
        );
    }
}
