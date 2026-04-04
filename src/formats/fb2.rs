use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use crate::formats::common::html_utils::{escape_html, strip_tags};
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
        reader.read_to_string(&mut contents)?;

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
                                        Some(String::from_utf8_lossy(&attr.value).into_owned());
                                },
                                b"content-type" => {
                                    current_binary_ctype =
                                        Some(String::from_utf8_lossy(&attr.value).into_owned());
                                },
                                _ => {},
                            }
                        }
                    }
                    if !path_buf.is_empty() {
                        path_buf.push('/');
                    }
                    path_buf.push_str(tag);
                    current_text.clear();
                },
                Ok(Event::Text(ref e)) => {
                    current_text = String::from_utf8_lossy(e.as_ref()).into_owned();
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
                        } else if path_buf
                            == "FictionBook/description/title-info/author/first-name"
                            || path_buf
                                == "FictionBook/description/title-info/author/last-name"
                            || path_buf
                                == "FictionBook/description/title-info/author/middle-name"
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
                        } else if path_buf
                            == "FictionBook/description/title-info/annotation/p"
                        {
                            let desc = book.metadata.description.get_or_insert_with(String::new);
                            if !desc.is_empty() {
                                desc.push('\n');
                            }
                            desc.push_str(&current_text);
                        }
                    } else {
                        // Parse content
                        if path_buf == "FictionBook/body/section/title/p" {
                            current_section_title = Some(current_text.clone());
                        } else if path_buf.starts_with("FictionBook/body/section") && tag == "p" {
                            current_section_content.push_str("<p>");
                            current_section_content.push_str(&current_text);
                            current_section_content.push_str("</p>\n");
                        } else if path_buf == "FictionBook/body/section" && tag == "section" {
                            section_counter += 1;
                            book.add_chapter(&Chapter {
                                title: current_section_title.take(),
                                content: current_section_content.clone(),
                                id: Some(format!("section_{}", section_counter)),
                            });
                            current_section_content.clear();
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

/// Generates a complete FictionBook 2.0 XML document from a `Book`.
fn generate_fb2(book: &Book) -> String {
    let mut xml = String::with_capacity(4096);

    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<FictionBook xmlns=\"http://www.gribuser.ru/xml/fictionbook/2.0\" ");
    xml.push_str("xmlns:l=\"http://www.w3.org/1999/xlink\">\n");

    // Description / title-info
    xml.push_str("  <description>\n");
    xml.push_str("    <title-info>\n");

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

    xml.push_str("    </title-info>\n");
    xml.push_str("  </description>\n");

    // Body
    xml.push_str("  <body>\n");
    for chapter in &book.chapters() {
        xml.push_str("    <section>\n");
        if let Some(ref ch_title) = chapter.title {
            xml.push_str("      <title><p>");
            xml.push_str(&escape_html(ch_title));
            xml.push_str("</p></title>\n");
        }
        // Convert HTML content to FB2 paragraphs.
        let plain = strip_tags(&chapter.content);
        for line in plain.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                xml.push_str("      <empty-line/>\n");
            } else {
                xml.push_str("      <p>");
                xml.push_str(&escape_html(trimmed));
                xml.push_str("</p>\n");
            }
        }
        xml.push_str("    </section>\n");
    }
    xml.push_str("  </body>\n");

    // Binary resources (base64-encoded)
    for resource in &book.resources() {
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
}
