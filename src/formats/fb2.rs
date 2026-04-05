use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use crate::formats::common::html_utils::escape_html;
use crate::formats::common::MAX_INPUT_SIZE;
use base64::Engine;
use quick_xml::Reader as XmlReader;
use quick_xml::events::Event;
use std::io::{Read, Write};

/// FB2 format reader.
#[derive(Default)]
pub struct Fb2Reader;

impl Fb2Reader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for Fb2Reader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut contents = String::new();
        (&mut *reader).take(MAX_INPUT_SIZE).read_to_string(&mut contents)?;

        if contents.trim().is_empty() {
            return Err(EruditioError::Format("Empty FB2 input".into()));
        }

        let mut xml_reader = XmlReader::from_str(&contents);
        xml_reader.config_mut().trim_text(true);

        let mut book = Book::new();
        let mut buf = Vec::new();

        // State tracking -- incremental path buffer avoids join("/") allocation per element.
        let mut path_buf = String::with_capacity(128);
        let mut current_text = String::new();
        let mut in_body = false;
        let mut current_section_title = None;
        let mut current_section_content = String::new();
        let mut section_counter: u32 = 0;
        // Track nested section depth within <body> so that content inside
        // `<section>` elements at any depth is captured, not just the first level.
        let mut section_depth: u32 = 0;
        let mut in_section_title = false;

        let mut current_binary_id = None;
        let mut current_binary_ctype = None;

        loop {
            match xml_reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name_raw = e.name();
                    let tag = std::str::from_utf8(name_raw.as_ref()).unwrap_or("");
                    if tag == "body" {
                        in_body = true;
                    } else if tag == "binary" {
                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"id" => {
                                    current_binary_id =
                                        Some(crate::formats::common::text_utils::bytes_to_string(
                                            &attr.value,
                                        ));
                                },
                                b"content-type" => {
                                    current_binary_ctype =
                                        Some(crate::formats::common::text_utils::bytes_to_string(
                                            &attr.value,
                                        ));
                                },
                                _ => {},
                            }
                        }
                    } else if tag == "section" && in_body {
                        // Entering a (possibly nested) section. If there is
                        // already accumulated content from the parent section,
                        // flush it as its own chapter before starting the child.
                        if section_depth > 0
                            && (current_section_title.is_some()
                                || !current_section_content.is_empty())
                        {
                            section_counter += 1;
                            book.add_chapter(&Chapter {
                                title: current_section_title.take(),
                                content: current_section_content.clone(),
                                id: Some(format!("section_{}", section_counter)),
                            });
                            current_section_content.clear();
                        }
                        section_depth += 1;
                    } else if tag == "title" && section_depth > 0 {
                        in_section_title = true;
                    }
                    if !path_buf.is_empty() {
                        path_buf.push('/');
                    }
                    path_buf.push_str(tag);
                    current_text.clear();
                },
                Ok(Event::Text(ref e)) => {
                    current_text = crate::formats::common::text_utils::bytes_to_string(e.as_ref());
                },
                Ok(Event::End(ref e)) => {
                    let name_raw = e.name();
                    let tag = std::str::from_utf8(name_raw.as_ref()).unwrap_or("");

                    if tag == "binary" {
                        if let Some(id) = current_binary_id.take() {
                            let decoded = base64::engine::general_purpose::STANDARD
                                .decode(current_text.trim().replace(['\n', '\r'], ""))
                                .unwrap_or_default();

                            let media_type = current_binary_ctype
                                .take()
                                .unwrap_or_else(|| "application/octet-stream".into());

                            let href = format!("images/{}", &id);
                            book.add_resource(&id, &href, decoded, media_type);
                        }
                    } else if !in_body {
                        // Parse metadata
                        if path_buf == "FictionBook/description/title-info/book-title" {
                            book.metadata.title = Some(current_text.clone());
                        } else if path_buf == "FictionBook/description/title-info/author/first-name"
                            || path_buf == "FictionBook/description/title-info/author/last-name"
                            || path_buf == "FictionBook/description/title-info/author/middle-name"
                        {
                            if tag == "first-name" {
                                book.metadata.authors.push(current_text.clone());
                            } else if tag == "last-name" || tag == "middle-name" {
                                if let Some(last) = book.metadata.authors.last_mut() {
                                    *last = format!("{} {}", last, current_text);
                                } else {
                                    book.metadata.authors.push(current_text.clone());
                                }
                            }
                        } else if path_buf == "FictionBook/description/title-info/lang" {
                            book.metadata.language = Some(current_text.clone());
                        } else if path_buf == "FictionBook/description/title-info/annotation/p" {
                            let desc = book.metadata.description.get_or_insert_with(String::new);
                            if !desc.is_empty() {
                                desc.push('\n');
                            }
                            desc.push_str(&current_text);
                        } else if path_buf == "FictionBook/description/publish-info/publisher" {
                            book.metadata.publisher = Some(current_text.clone());
                        } else if path_buf == "FictionBook/description/publish-info/isbn" {
                            book.metadata.isbn = Some(current_text.clone());
                        } else if path_buf == "FictionBook/description/publish-info/year" {
                            if let Ok(year) = current_text.trim().parse::<i32>() {
                                use chrono::NaiveDate;
                                if let Some(date) = NaiveDate::from_ymd_opt(year, 1, 1) {
                                    book.metadata.publication_date =
                                        Some(date.and_hms_opt(0, 0, 0).unwrap().and_utc());
                                }
                            }
                        }
                    } else {
                        // Parse content
                        if tag == "p" && in_section_title {
                            current_section_title = Some(current_text.clone());
                        } else if tag == "title" && section_depth > 0 {
                            in_section_title = false;
                        } else if section_depth > 0 && tag == "p" {
                            current_section_content.push_str("<p>");
                            current_section_content.push_str(&current_text);
                            current_section_content.push_str("</p>\n");
                        } else if tag == "section" && section_depth > 0 {
                            section_depth -= 1;
                            // Only emit a chapter when there is a title or
                            // content. This avoids empty chapters for wrapper
                            // sections whose content was already flushed when
                            // their child sections started.
                            if current_section_title.is_some()
                                || !current_section_content.is_empty()
                            {
                                section_counter += 1;
                                book.add_chapter(&Chapter {
                                    title: current_section_title.take(),
                                    content: current_section_content.clone(),
                                    id: Some(format!("section_{}", section_counter)),
                                });
                                current_section_content.clear();
                            }
                        } else if tag == "body" {
                            in_body = false;
                        }
                    }

                    // Truncate path_buf back to parent.
                    if let Some(pos) = path_buf.rfind('/') {
                        path_buf.truncate(pos);
                    } else {
                        path_buf.clear();
                    }
                    current_text.clear();
                },
                Ok(Event::Empty(ref e)) => {
                    if in_body && e.name().as_ref() == b"empty-line" {
                        current_section_content.push_str("<br/>\n");
                    }
                },
                Ok(Event::Eof) => break,
                Err(e) => return Err(EruditioError::Parse(format!("XML error: {}", e))),
                _ => (),
            }
            buf.clear();
        }

        Ok(book)
    }
}

/// FB2 format writer.
#[derive(Default)]
pub struct Fb2Writer;

impl Fb2Writer {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for Fb2Writer {
    fn write_book(&self, book: &Book, writer: &mut dyn Write) -> Result<()> {
        let xml = generate_fb2(book);
        writer.write_all(xml.as_bytes())?;
        Ok(())
    }
}

/// Converts HTML content into FB2 paragraph elements.
///
/// - Wraps text inside `<p>` tags as FB2 `<p>` elements.
/// - Converts `<a href="...">text</a>` to `<a l:href="...">text</a>`.
/// - Emits `<empty-line/>` only for explicit `<br>` / `<br/>` tags in the source,
///   NOT after every paragraph boundary.
/// - Text outside any `<p>` is treated as implicit paragraphs (split by newlines).
fn html_to_fb2_paragraphs(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut pos = 0;

    // We accumulate inline content (text + converted links) per paragraph.
    // When we encounter </p> or end-of-input, we flush the paragraph.
    let mut inline_buf = String::new();
    let mut in_p = false;
    let mut in_anchor = false;

    while pos < len {
        if bytes[pos] == b'<' {
            // Parse the tag
            if let Some(gt) = memchr::memchr(b'>', &bytes[pos..]) {
                let tag_bytes = &bytes[pos..pos + gt + 1];
                let tag_str = std::str::from_utf8(tag_bytes).unwrap_or("");
                let tag_lower = tag_str.to_ascii_lowercase();

                if tag_lower.starts_with("<p") && (tag_bytes.len() < 3 || tag_bytes[2] == b'>' || tag_bytes[2] == b' ') {
                    // Opening <p> tag – start accumulating inline content
                    if in_anchor {
                        inline_buf.push_str("</a>");
                        in_anchor = false;
                    }
                    flush_paragraph(&mut out, &mut inline_buf);
                    in_p = true;
                    pos += gt + 1;
                } else if tag_lower.starts_with("</p") {
                    // Closing </p> tag – flush current paragraph
                    if in_anchor {
                        inline_buf.push_str("</a>");
                        in_anchor = false;
                    }
                    flush_paragraph(&mut out, &mut inline_buf);
                    in_p = false;
                    pos += gt + 1;
                } else if tag_lower.starts_with("<br") {
                    // <br> or <br/> – emit empty-line in FB2
                    if in_anchor {
                        inline_buf.push_str("</a>");
                        in_anchor = false;
                    }
                    flush_paragraph(&mut out, &mut inline_buf);
                    out.push_str("      <empty-line/>\n");
                    pos += gt + 1;
                } else if tag_lower.starts_with("<a ") || tag_lower.starts_with("<a>") {
                    // Opening <a> tag – extract href and convert to l:href
                    if let Some(href) = extract_href(tag_str) {
                        // Only emit link for external URLs; internal EPUB references
                        // are meaningless in FB2 context
                        if is_external_url(&href) {
                            inline_buf.push_str("<a l:href=\"");
                            inline_buf.push_str(&escape_html(&href));
                            inline_buf.push_str("\">");
                            in_anchor = true;
                        }
                        // else: skip the <a> tag, text content will flow through as plain text
                    }
                    // If no href, just skip the tag (keep the text content)
                    pos += gt + 1;
                } else if tag_lower.starts_with("</a") {
                    // Closing </a> tag
                    if in_anchor {
                        inline_buf.push_str("</a>");
                        in_anchor = false;
                    }
                    pos += gt + 1;
                } else if tag_lower == "<b>" || tag_lower == "<strong>"
                    || tag_lower.starts_with("<b ") || tag_lower.starts_with("<strong ") {
                    // Opening bold tag → FB2 <strong>
                    inline_buf.push_str("<strong>");
                    pos += gt + 1;
                } else if tag_lower == "</b>" || tag_lower == "</strong>" {
                    // Closing bold tag
                    inline_buf.push_str("</strong>");
                    pos += gt + 1;
                } else if tag_lower == "<i>" || tag_lower == "<em>"
                    || tag_lower.starts_with("<i ") || tag_lower.starts_with("<em ") {
                    // Opening italic tag → FB2 <emphasis>
                    inline_buf.push_str("<emphasis>");
                    pos += gt + 1;
                } else if tag_lower == "</i>" || tag_lower == "</em>" {
                    // Closing italic tag
                    inline_buf.push_str("</emphasis>");
                    pos += gt + 1;
                } else {
                    // Other tags (e.g. <div>, <span>, etc.) – skip the tag, keep going
                    pos += gt + 1;
                }
            } else {
                // Unclosed '<' – treat as text
                inline_buf.push_str(&escape_html(&html[pos..pos + 1]));
                pos += 1;
            }
        } else {
            // Regular text content
            let next_lt = memchr::memchr(b'<', &bytes[pos..]).unwrap_or(len - pos);
            let text = &html[pos..pos + next_lt];
            if in_p {
                // Inside a <p>, accumulate text
                inline_buf.push_str(&escape_html(text));
            } else {
                // Outside <p>: treat non-empty lines as paragraphs
                for line in text.split('\n') {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        inline_buf.push_str(&escape_html(trimmed));
                        if in_anchor {
                            inline_buf.push_str("</a>");
                            in_anchor = false;
                        }
                        flush_paragraph(&mut out, &mut inline_buf);
                    }
                }
            }
            pos += next_lt;
        }
    }

    // Flush any trailing inline content
    if in_anchor {
        inline_buf.push_str("</a>");
        // in_anchor = false; // not needed, end of function
    }
    flush_paragraph(&mut out, &mut inline_buf);
    out
}

/// If the inline buffer has content, wrap it in `<p>...</p>` and append to `out`.
fn flush_paragraph(out: &mut String, inline_buf: &mut String) {
    let trimmed = inline_buf.trim();
    if !trimmed.is_empty() {
        out.push_str("      <p>");
        out.push_str(trimmed);
        out.push_str("</p>\n");
    }
    inline_buf.clear();
}

/// Extracts the `href` attribute value from an `<a ...>` tag string.
fn extract_href(tag: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let href_pos = lower.find("href=")?;
    let after_eq = href_pos + 5; // length of "href="
    let bytes = tag.as_bytes();
    if after_eq >= bytes.len() {
        return None;
    }
    let quote = bytes[after_eq];
    if quote == b'"' || quote == b'\'' {
        let start = after_eq + 1;
        let end = memchr::memchr(quote, &bytes[start..])?;
        Some(tag[start..start + end].to_string())
    } else {
        // Unquoted value – take until whitespace or '>'
        let start = after_eq;
        let end = tag[start..]
            .find(|c: char| c.is_whitespace() || c == '>')
            .unwrap_or(tag.len() - start);
        Some(tag[start..start + end].to_string())
    }
}

/// Returns true if the URL is an external link (http, https, ftp, mailto).
fn is_external_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ftp://")
        || lower.starts_with("mailto:")
}

/// Generates a complete FictionBook 2.0 XML document from a `Book`.
fn generate_fb2(book: &Book) -> String {
    let mut xml = String::with_capacity(4096);

    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<FictionBook xmlns=\"http://www.gribuser.ru/xml/fictionbook/2.0\" ");
    xml.push_str("xmlns:l=\"http://www.w3.org/1999/xlink\">\n");

    // Description / title-info
    xml.push_str("  <description>\n");
    xml.push_str("    <title-info>\n");

    // Genre (FB2 requires at least one)
    if let Some(subject) = book.metadata.subjects.first() {
        xml.push_str("      <genre>");
        xml.push_str(&escape_html(subject));
        xml.push_str("</genre>\n");
    } else {
        xml.push_str("      <genre>other</genre>\n");
    }

    // Authors
    for author in &book.metadata.authors {
        xml.push_str("      <author>\n");
        let parts: Vec<&str> = author.splitn(2, ' ').collect();
        if parts.len() == 2 {
            xml.push_str("        <first-name>");
            xml.push_str(&escape_html(parts[0]));
            xml.push_str("</first-name>\n");
            xml.push_str("        <last-name>");
            xml.push_str(&escape_html(parts[1]));
            xml.push_str("</last-name>\n");
        } else {
            xml.push_str("        <first-name>");
            xml.push_str(&escape_html(author));
            xml.push_str("</first-name>\n");
        }
        xml.push_str("      </author>\n");
    }
    if book.metadata.authors.is_empty() {
        xml.push_str("      <author><first-name>Unknown</first-name></author>\n");
    }

    // Title
    let title = book.metadata.title.as_deref().unwrap_or("Untitled");
    xml.push_str("      <book-title>");
    xml.push_str(&escape_html(title));
    xml.push_str("</book-title>\n");

    // Language
    if let Some(ref lang) = book.metadata.language {
        xml.push_str("      <lang>");
        xml.push_str(&escape_html(lang));
        xml.push_str("</lang>\n");
    }

    // Annotation (description)
    if let Some(ref desc) = book.metadata.description {
        xml.push_str("      <annotation>\n");
        for line in desc.lines() {
            xml.push_str("        <p>");
            xml.push_str(&escape_html(line));
            xml.push_str("</p>\n");
        }
        xml.push_str("      </annotation>\n");
    }

    // Coverpage – look for a cover image in the manifest
    let cover_id = book
        .manifest
        .iter()
        .find(|item| {
            item.id.to_lowercase().contains("cover")
                && item.media_type.starts_with("image/")
        })
        .map(|item| item.id.clone());
    if let Some(ref cid) = cover_id {
        xml.push_str("      <coverpage><image l:href=\"#");
        xml.push_str(&escape_html(cid));
        xml.push_str("\"/></coverpage>\n");
    }

    xml.push_str("    </title-info>\n");

    // Document-info (metadata about this conversion)
    xml.push_str("    <document-info>\n");
    xml.push_str("      <program-used>eruditio</program-used>\n");
    xml.push_str("      <date>");
    xml.push_str(&chrono::Utc::now().format("%Y-%m-%d").to_string());
    xml.push_str("</date>\n");
    xml.push_str("    </document-info>\n");

    // Publish-info (publisher, isbn, year)
    let has_publisher = book.metadata.publisher.is_some();
    let has_isbn = book.metadata.isbn.is_some();
    let has_pub_date = book.metadata.publication_date.is_some();
    if has_publisher || has_isbn || has_pub_date {
        xml.push_str("    <publish-info>\n");
        if let Some(ref publisher) = book.metadata.publisher {
            xml.push_str("      <publisher>");
            xml.push_str(&escape_html(publisher));
            xml.push_str("</publisher>\n");
        }
        if let Some(ref isbn) = book.metadata.isbn {
            xml.push_str("      <isbn>");
            xml.push_str(&escape_html(isbn));
            xml.push_str("</isbn>\n");
        }
        if let Some(ref pub_date) = book.metadata.publication_date {
            xml.push_str("      <year>");
            xml.push_str(&pub_date.format("%Y").to_string());
            xml.push_str("</year>\n");
        }
        xml.push_str("    </publish-info>\n");
    }

    xml.push_str("  </description>\n");

    // Body
    xml.push_str("  <body>\n");

    // Add cover image section if a cover exists
    if let Some(ref cid) = cover_id {
        xml.push_str("    <section>\n");
        xml.push_str("      <image l:href=\"#");
        xml.push_str(&escape_html(cid));
        xml.push_str("\"/>\n");
        xml.push_str("    </section>\n");
    }

    for chapter in &book.chapters() {
        xml.push_str("    <section>\n");
        if let Some(ref ch_title) = chapter.title {
            xml.push_str("      <title><p>");
            xml.push_str(&escape_html(ch_title));
            xml.push_str("</p></title>\n");
        }
        // Convert HTML content to FB2 paragraphs.
        let fb2_content = html_to_fb2_paragraphs(&chapter.content);
        xml.push_str(&fb2_content);
        xml.push_str("    </section>\n");
    }
    xml.push_str("  </body>\n");

    // Binary resources (base64-encoded)
    for resource in &book.resources() {
        // Skip CSS resources — FB2 readers don't use CSS
        if resource.media_type == "text/css" {
            continue;
        }
        xml.push_str("  <binary id=\"");
        xml.push_str(&escape_html(resource.id));
        xml.push_str("\" content-type=\"");
        xml.push_str(&escape_html(resource.media_type));
        xml.push_str("\">");
        let b64 = base64::engine::general_purpose::STANDARD.encode(resource.data);
        xml.push_str(&b64);
        xml.push_str("</binary>\n");
    }

    xml.push_str("</FictionBook>\n");
    xml
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn fb2_writer_produces_valid_xml() {
        let mut book = Book::new();
        book.metadata.title = Some("Test Book".into());
        book.metadata.authors.push("Jane Doe".into());
        book.metadata.language = Some("en".into());

        book.add_chapter(&Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello world</p>".into(),
            id: Some("ch1".into()),
        });

        book.add_resource("img1", "images/test.jpg", vec![0xFF, 0xD8], "image/jpeg");

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(xml.contains("<book-title>Test Book</book-title>"));
        assert!(xml.contains("<first-name>Jane</first-name>"));
        assert!(xml.contains("<last-name>Doe</last-name>"));
        assert!(xml.contains("<lang>en</lang>"));
        assert!(xml.contains("<p>Hello world</p>"));
        assert!(xml.contains("content-type=\"image/jpeg\""));
        assert!(xml.contains("id=\"img1\""));
    }

    #[test]
    fn fb2_round_trip_preserves_content() {
        let mut book = Book::new();
        book.metadata.title = Some("Round Trip".into());
        book.metadata.authors.push("Author Name".into());

        book.add_chapter(&Chapter {
            title: Some("Section One".into()),
            content: "<p>First paragraph</p><p>Second paragraph</p>".into(),
            id: Some("s1".into()),
        });

        // Write to FB2
        let mut fb2_bytes = Vec::new();
        Fb2Writer::new().write_book(&book, &mut fb2_bytes).unwrap();

        // Read back
        let mut cursor = Cursor::new(fb2_bytes);
        let decoded = Fb2Reader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("Round Trip"));
        let chapters = decoded.chapters();
        assert!(!chapters.is_empty());
        assert_eq!(chapters[0].title.as_deref(), Some("Section One"));
    }

    #[test]
    fn fb2_writer_generates_publish_info() {
        use chrono::NaiveDate;

        let mut book = Book::new();
        book.metadata.title = Some("Test Book".into());
        book.metadata.publisher = Some("Test Press".into());
        book.metadata.isbn = Some("978-0-123456-78-9".into());
        book.metadata.publication_date = Some(
            NaiveDate::from_ymd_opt(2024, 6, 15)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
        );

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(xml.contains("<publish-info>"), "missing <publish-info>");
        assert!(
            xml.contains("<publisher>Test Press</publisher>"),
            "missing publisher"
        );
        assert!(
            xml.contains("<isbn>978-0-123456-78-9</isbn>"),
            "missing isbn"
        );
        assert!(xml.contains("<year>2024</year>"), "missing year");
        assert!(
            xml.contains("</publish-info>"),
            "missing </publish-info>"
        );
    }

    #[test]
    fn fb2_writer_omits_publish_info_when_empty() {
        let mut book = Book::new();
        book.metadata.title = Some("No Publish Info".into());

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            !xml.contains("<publish-info>"),
            "publish-info should not be present when all fields are None"
        );
    }

    #[test]
    fn fb2_writer_partial_publish_info() {
        let mut book = Book::new();
        book.metadata.title = Some("Partial".into());
        book.metadata.publisher = Some("Only Publisher".into());
        // isbn and publication_date are None

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(xml.contains("<publish-info>"));
        assert!(xml.contains("<publisher>Only Publisher</publisher>"));
        assert!(!xml.contains("<isbn>"));
        assert!(!xml.contains("<year>"));
    }

    #[test]
    fn fb2_reader_parses_publish_info() {
        let fb2_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0">
  <description>
    <title-info>
      <book-title>Parsed Book</book-title>
    </title-info>
    <publish-info>
      <publisher>Acme Publishing</publisher>
      <isbn>978-1-234567-89-0</isbn>
      <year>2023</year>
    </publish-info>
  </description>
  <body>
    <section>
      <title><p>Ch1</p></title>
      <p>Content here</p>
    </section>
  </body>
</FictionBook>"#;

        let mut cursor = Cursor::new(fb2_xml.as_bytes());
        let book = Fb2Reader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Parsed Book"));
        assert_eq!(
            book.metadata.publisher.as_deref(),
            Some("Acme Publishing")
        );
        assert_eq!(
            book.metadata.isbn.as_deref(),
            Some("978-1-234567-89-0")
        );
        assert!(book.metadata.publication_date.is_some());
        assert_eq!(
            book.metadata.publication_date.unwrap().format("%Y").to_string(),
            "2023"
        );
    }

    #[test]
    fn fb2_publish_info_round_trip() {
        use chrono::NaiveDate;

        let mut book = Book::new();
        book.metadata.title = Some("Round Trip Publish".into());
        book.metadata.authors.push("Jane Doe".into());
        book.metadata.publisher = Some("Test Press".into());
        book.metadata.isbn = Some("978-0-123456-78-9".into());
        book.metadata.publication_date = Some(
            NaiveDate::from_ymd_opt(2024, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
        );

        book.add_chapter(&Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        // Write
        let mut fb2_bytes = Vec::new();
        Fb2Writer::new().write_book(&book, &mut fb2_bytes).unwrap();

        // Read back
        let mut cursor = Cursor::new(fb2_bytes);
        let decoded = Fb2Reader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.publisher.as_deref(), Some("Test Press"));
        assert_eq!(
            decoded.metadata.isbn.as_deref(),
            Some("978-0-123456-78-9")
        );
        assert!(decoded.metadata.publication_date.is_some());
        assert_eq!(
            decoded
                .metadata
                .publication_date
                .unwrap()
                .format("%Y")
                .to_string(),
            "2024"
        );
    }

    // =========================================================================
    // New tests for the 5 FB2 writer enhancements
    // =========================================================================

    #[test]
    fn fb2_writer_includes_document_info() {
        let mut book = Book::new();
        book.metadata.title = Some("Doc Info Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains("<document-info>"),
            "missing <document-info>"
        );
        assert!(
            xml.contains("<program-used>eruditio</program-used>"),
            "missing program-used"
        );
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let expected_date = format!("<date>{}</date>", today);
        assert!(
            xml.contains(&expected_date),
            "missing current date in document-info, expected {}, got:\n{}",
            expected_date,
            xml
        );
        assert!(
            xml.contains("</document-info>"),
            "missing </document-info>"
        );

        // Verify ordering: document-info comes after title-info and before publish-info / </description>
        let ti_end = xml.find("</title-info>").expect("no </title-info>");
        let di_start = xml.find("<document-info>").expect("no <document-info>");
        let desc_end = xml.find("</description>").expect("no </description>");
        assert!(
            di_start > ti_end,
            "document-info should come after title-info"
        );
        assert!(
            di_start < desc_end,
            "document-info should come before </description>"
        );
    }

    #[test]
    fn fb2_writer_includes_genre_from_subjects() {
        let mut book = Book::new();
        book.metadata.title = Some("Genre Test".into());
        book.metadata.subjects.push("science_fiction".into());
        book.metadata.subjects.push("adventure".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains("<genre>science_fiction</genre>"),
            "should use first subject as genre, got: {}",
            xml
        );
        // Genre should appear before <author>
        let genre_pos = xml.find("<genre>").unwrap();
        let author_pos = xml.find("<author>").unwrap();
        assert!(genre_pos < author_pos, "genre should appear before author");
    }

    #[test]
    fn fb2_writer_includes_default_genre_when_no_subjects() {
        let mut book = Book::new();
        book.metadata.title = Some("Default Genre".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains("<genre>other</genre>"),
            "should use 'other' as default genre"
        );
    }

    #[test]
    fn fb2_writer_includes_coverpage_when_cover_image_exists() {
        let mut book = Book::new();
        book.metadata.title = Some("Cover Test".into());
        book.add_resource("cover", "images/cover.jpg", vec![0xFF, 0xD8], "image/jpeg");
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains("<coverpage><image l:href=\"#cover\"/></coverpage>"),
            "missing coverpage element, got:\n{}",
            xml
        );
        // Coverpage should be inside title-info
        let ti_start = xml.find("<title-info>").unwrap();
        let ti_end = xml.find("</title-info>").unwrap();
        let cp_pos = xml.find("<coverpage>").unwrap();
        assert!(cp_pos > ti_start && cp_pos < ti_end, "coverpage should be inside title-info");
    }

    #[test]
    fn fb2_writer_omits_coverpage_when_no_cover_image() {
        let mut book = Book::new();
        book.metadata.title = Some("No Cover".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            !xml.contains("<coverpage>"),
            "coverpage should not be present without a cover image"
        );
    }

    #[test]
    fn fb2_writer_preserves_inline_formatting() {
        let mut book = Book::new();
        book.metadata.title = Some("Formatting Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>This is <b>bold</b> and <i>italic</i> text.</p><p>Also <strong>strong</strong> and <em>emphasis</em>.</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains("<strong>bold</strong>"),
            "HTML <b> should become FB2 <strong>, got:\n{xml}"
        );
        assert!(
            xml.contains("<emphasis>italic</emphasis>"),
            "HTML <i> should become FB2 <emphasis>, got:\n{xml}"
        );
        assert!(
            xml.contains("<strong>strong</strong>"),
            "HTML <strong> should become FB2 <strong>, got:\n{xml}"
        );
        assert!(
            xml.contains("<emphasis>emphasis</emphasis>"),
            "HTML <em> should become FB2 <emphasis>, got:\n{xml}"
        );
    }

    #[test]
    fn fb2_writer_converts_hyperlinks() {
        let mut book = Book::new();
        book.metadata.title = Some("Link Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: r#"<p>Click <a href="http://example.com">here</a> for more.</p>"#.into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains(r#"<a l:href="http://example.com">here</a>"#),
            "hyperlinks should be converted to l:href format, got:\n{}",
            xml
        );
        assert!(
            xml.contains("Click "),
            "text before link should be preserved"
        );
        assert!(
            xml.contains(" for more."),
            "text after link should be preserved"
        );
    }

    #[test]
    fn fb2_writer_closes_anchor_at_paragraph_boundary() {
        let mut book = Book::new();
        book.metadata.title = Some("Anchor Close Test".into());
        // Simulate a link that spans across a paragraph boundary:
        // the </a> comes after the </p>, so the writer must auto-close it.
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: r#"<p><a href="https://example.org">link text</p><p>next paragraph</p>"#.into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // The anchor must be closed before the paragraph closes
        assert!(
            xml.contains(r#"<a l:href="https://example.org">link text</a></p>"#),
            "anchor tag should be closed before </p>, got:\n{}",
            xml
        );
        // The output must not contain an unclosed <a> tag
        assert!(
            !xml.contains(r#"<a l:href="https://example.org">link text</p>"#),
            "must not have unclosed <a> tag, got:\n{}",
            xml
        );
        // Validate the XML is well-formed
        assert!(
            xml.contains("next paragraph"),
            "subsequent paragraph content should be preserved"
        );
    }

    #[test]
    fn fb2_writer_no_excessive_empty_lines() {
        let mut book = Book::new();
        book.metadata.title = Some("Empty Line Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>First paragraph</p><p>Second paragraph</p><p>Third paragraph</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        let empty_line_count = xml.matches("<empty-line/>").count();
        assert_eq!(
            empty_line_count, 0,
            "consecutive <p> tags should NOT produce empty-lines, but found {}",
            empty_line_count
        );
        // All three paragraphs should be present
        assert!(xml.contains("<p>First paragraph</p>"));
        assert!(xml.contains("<p>Second paragraph</p>"));
        assert!(xml.contains("<p>Third paragraph</p>"));
    }

    #[test]
    fn fb2_writer_emits_empty_line_for_br() {
        let mut book = Book::new();
        book.metadata.title = Some("BR Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Before break</p><br/><p>After break</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        let empty_line_count = xml.matches("<empty-line/>").count();
        assert_eq!(
            empty_line_count, 1,
            "a <br/> between paragraphs should produce exactly one empty-line, got {}",
            empty_line_count
        );
    }

    // =========================================================================
    // Tests for nested section handling in FB2 reader
    // =========================================================================

    #[test]
    fn fb2_reader_nested_sections() {
        let fb2_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0">
  <description>
    <title-info>
      <book-title>Nested Test</book-title>
    </title-info>
  </description>
  <body>
    <section>
      <title><p>Chapter 1</p></title>
      <section>
        <title><p>Section 1.1</p></title>
        <p>Content of 1.1</p>
      </section>
      <section>
        <title><p>Section 1.2</p></title>
        <p>Content of 1.2</p>
      </section>
    </section>
  </body>
</FictionBook>"#;

        let mut cursor = Cursor::new(fb2_xml.as_bytes());
        let book = Fb2Reader::new().read_book(&mut cursor).unwrap();
        let chapters = book.chapters();

        // The outer section has a title but the content was flushed when the
        // first child section started, producing a chapter for "Chapter 1"
        // (empty body) and one chapter per inner section.
        assert!(
            chapters.len() >= 2,
            "expected at least 2 chapters for nested sections, got {}",
            chapters.len()
        );

        // Find chapters by title
        let titles: Vec<Option<&str>> = chapters.iter().map(|c| c.title.as_deref()).collect();
        assert!(
            titles.contains(&Some("Section 1.1")),
            "missing 'Section 1.1' chapter, found titles: {:?}",
            titles
        );
        assert!(
            titles.contains(&Some("Section 1.2")),
            "missing 'Section 1.2' chapter, found titles: {:?}",
            titles
        );

        // Verify inner section content is not dropped
        let sec11 = chapters.iter().find(|c| c.title.as_deref() == Some("Section 1.1")).unwrap();
        assert!(
            sec11.content.contains("Content of 1.1"),
            "Section 1.1 content was dropped: {:?}",
            sec11.content
        );
        let sec12 = chapters.iter().find(|c| c.title.as_deref() == Some("Section 1.2")).unwrap();
        assert!(
            sec12.content.contains("Content of 1.2"),
            "Section 1.2 content was dropped: {:?}",
            sec12.content
        );
    }

    #[test]
    fn fb2_reader_deeply_nested_sections() {
        let fb2_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0">
  <description>
    <title-info>
      <book-title>Deep Nesting</book-title>
    </title-info>
  </description>
  <body>
    <section>
      <title><p>Part I</p></title>
      <section>
        <title><p>Chapter 1</p></title>
        <section>
          <title><p>Section 1.1</p></title>
          <p>Deep content here</p>
        </section>
      </section>
    </section>
  </body>
</FictionBook>"#;

        let mut cursor = Cursor::new(fb2_xml.as_bytes());
        let book = Fb2Reader::new().read_book(&mut cursor).unwrap();
        let chapters = book.chapters();

        let titles: Vec<Option<&str>> = chapters.iter().map(|c| c.title.as_deref()).collect();
        assert!(
            titles.contains(&Some("Section 1.1")),
            "deeply nested section title not found, got: {:?}",
            titles
        );

        let sec = chapters.iter().find(|c| c.title.as_deref() == Some("Section 1.1")).unwrap();
        assert!(
            sec.content.contains("Deep content here"),
            "deeply nested section content was dropped: {:?}",
            sec.content
        );
    }

    #[test]
    fn fb2_reader_flat_sections_still_work() {
        // Regression test: flat (non-nested) sections must keep working.
        let fb2_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0">
  <description>
    <title-info>
      <book-title>Flat Sections</book-title>
    </title-info>
  </description>
  <body>
    <section>
      <title><p>Chapter 1</p></title>
      <p>First chapter content</p>
    </section>
    <section>
      <title><p>Chapter 2</p></title>
      <p>Second chapter content</p>
    </section>
    <section>
      <title><p>Chapter 3</p></title>
      <p>Third chapter content</p>
    </section>
  </body>
</FictionBook>"#;

        let mut cursor = Cursor::new(fb2_xml.as_bytes());
        let book = Fb2Reader::new().read_book(&mut cursor).unwrap();
        let chapters = book.chapters();

        assert_eq!(
            chapters.len(),
            3,
            "expected 3 flat chapters, got {}",
            chapters.len()
        );
        assert_eq!(chapters[0].title.as_deref(), Some("Chapter 1"));
        assert_eq!(chapters[1].title.as_deref(), Some("Chapter 2"));
        assert_eq!(chapters[2].title.as_deref(), Some("Chapter 3"));
        assert!(chapters[0].content.contains("First chapter content"));
        assert!(chapters[1].content.contains("Second chapter content"));
        assert!(chapters[2].content.contains("Third chapter content"));
    }

    #[test]
    fn fb2_reader_nested_section_with_parent_content() {
        // A parent section has content before its nested child sections.
        let fb2_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0">
  <description>
    <title-info>
      <book-title>Mixed Content</book-title>
    </title-info>
  </description>
  <body>
    <section>
      <title><p>Introduction</p></title>
      <p>Intro paragraph</p>
      <section>
        <title><p>Details</p></title>
        <p>Detail paragraph</p>
      </section>
    </section>
  </body>
</FictionBook>"#;

        let mut cursor = Cursor::new(fb2_xml.as_bytes());
        let book = Fb2Reader::new().read_book(&mut cursor).unwrap();
        let chapters = book.chapters();

        // The parent section's content should be flushed as a chapter before
        // the child section starts.
        assert!(
            chapters.len() >= 2,
            "expected at least 2 chapters, got {}",
            chapters.len()
        );

        let intro = chapters.iter().find(|c| c.title.as_deref() == Some("Introduction")).unwrap();
        assert!(
            intro.content.contains("Intro paragraph"),
            "parent section content was lost: {:?}",
            intro.content
        );

        let details = chapters.iter().find(|c| c.title.as_deref() == Some("Details")).unwrap();
        assert!(
            details.content.contains("Detail paragraph"),
            "child section content was lost: {:?}",
            details.content
        );
    }

    #[test]
    fn fb2_writer_strips_internal_epub_links() {
        let mut book = Book::new();
        book.metadata.title = Some("Internal Link Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: r#"<p>See <a href="@public@vhost@g@gutenberg@html@files@11@11-h@11-h-0.htm.html#link2HCH0001">Chapter 1</a> for details.</p>"#.into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // The link wrapper should be stripped; text content preserved
        assert!(
            !xml.contains("<a "),
            "internal EPUB links should be stripped, but found <a> tag in:\n{}",
            xml
        );
        assert!(
            xml.contains("Chapter 1"),
            "link text should be preserved as inline content"
        );
        assert!(
            xml.contains("See "),
            "text before link should be preserved"
        );
        assert!(
            xml.contains(" for details."),
            "text after link should be preserved"
        );
    }

    #[test]
    fn fb2_writer_strips_fragment_only_links() {
        let mut book = Book::new();
        book.metadata.title = Some("Fragment Link Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: r##"<p>Go to <a href="#section1">Section 1</a> now.</p>"##.into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            !xml.contains("<a "),
            "fragment-only links should be stripped, but found <a> tag in:\n{}",
            xml
        );
        assert!(
            xml.contains("Section 1"),
            "link text should be preserved as inline content"
        );
    }

    #[test]
    fn fb2_writer_preserves_external_links() {
        // Verify that http/https/ftp/mailto links are still emitted
        let mut book = Book::new();
        book.metadata.title = Some("External Link Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: r#"<p><a href="https://example.com">HTTPS</a> <a href="http://example.com">HTTP</a> <a href="ftp://files.example.com">FTP</a> <a href="mailto:test@example.com">Email</a></p>"#.into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains(r#"<a l:href="https://example.com">HTTPS</a>"#),
            "https links should be preserved"
        );
        assert!(
            xml.contains(r#"<a l:href="http://example.com">HTTP</a>"#),
            "http links should be preserved"
        );
        assert!(
            xml.contains(r#"<a l:href="ftp://files.example.com">FTP</a>"#),
            "ftp links should be preserved"
        );
        assert!(
            xml.contains(r#"<a l:href="mailto:test@example.com">Email</a>"#),
            "mailto links should be preserved"
        );
    }

    #[test]
    fn fb2_writer_strips_relative_path_links() {
        let mut book = Book::new();
        book.metadata.title = Some("Relative Link Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: r#"<p>See <a href="chapter1.xhtml#section1">this section</a> please.</p>"#.into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            !xml.contains("<a "),
            "relative path links should be stripped, got:\n{}",
            xml
        );
        assert!(
            xml.contains("this section"),
            "link text should be preserved"
        );
    }

    // =========================================================================
    // Tests for CSS filtering and cover image in body
    // =========================================================================

    #[test]
    fn fb2_writer_excludes_css_from_binary_elements() {
        let mut book = Book::new();
        book.metadata.title = Some("CSS Filter Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        // Add a CSS resource and an image resource
        book.add_resource("style1", "styles/main.css", b"body { color: red; }".to_vec(), "text/css");
        book.add_resource("img1", "images/photo.jpg", vec![0xFF, 0xD8], "image/jpeg");

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // CSS should NOT appear as a binary element
        assert!(
            !xml.contains("id=\"style1\""),
            "CSS resource should be filtered out of binary elements, got:\n{}",
            xml
        );
        assert!(
            !xml.contains("content-type=\"text/css\""),
            "text/css content-type should not appear in binary elements"
        );

        // Image resource should still be present
        assert!(
            xml.contains("id=\"img1\""),
            "image resource should still be included as binary element"
        );
        assert!(
            xml.contains("content-type=\"image/jpeg\""),
            "image content-type should be present"
        );
    }

    #[test]
    fn fb2_writer_cover_image_in_body() {
        let mut book = Book::new();
        book.metadata.title = Some("Cover Body Test".into());
        book.add_resource("cover", "images/cover.jpg", vec![0xFF, 0xD8], "image/jpeg");
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // Extract the body section
        let body_start = xml.find("<body>").expect("missing <body>");
        let body_end = xml.find("</body>").expect("missing </body>");
        let body_section = &xml[body_start..body_end];

        // The body should contain an image element referencing the cover
        assert!(
            body_section.contains("<image l:href=\"#cover\"/>"),
            "body should contain cover image reference, got:\n{}",
            body_section
        );

        // The cover image section should appear before chapter sections
        let cover_in_body = body_section.find("<image l:href=\"#cover\"/>").unwrap();
        let chapter_in_body = body_section.find("<title><p>Ch1</p></title>").expect("missing chapter title in body");
        assert!(
            cover_in_body < chapter_in_body,
            "cover image should appear before chapter content in body"
        );
    }

    #[test]
    fn fb2_writer_no_cover_image_in_body_without_cover() {
        let mut book = Book::new();
        book.metadata.title = Some("No Cover Body Test".into());
        // Add a non-cover image resource
        book.add_resource("img1", "images/photo.jpg", vec![0xFF, 0xD8], "image/jpeg");
        book.add_chapter(&Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // Should not have a cover image section in the body
        let body_section = &xml[xml.find("<body>").unwrap()..xml.find("</body>").unwrap()];
        assert!(
            !body_section.contains("<image l:href=\"#"),
            "body should not contain cover image section when no cover exists, got:\n{}",
            body_section
        );
    }
}
