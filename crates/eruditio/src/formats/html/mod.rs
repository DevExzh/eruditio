//! HTML format reader and writer.
//!
//! Reads HTML files into the `Book` intermediate representation and writes
//! books back as standalone HTML documents.

pub mod parser;

use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::Result;
use crate::formats::common::html_utils::strip_leading_heading;
use crate::formats::common::text_utils::push_escape_html;
use std::borrow::Cow;
use std::io::{Read, Write};

/// HTML format reader.
///
/// Parses an HTML document, extracting metadata from `<head>` and content
/// from `<body>`. Splits content into chapters at `<h1>`/`<h2>` boundaries.
#[derive(Default)]
pub struct HtmlReader;

impl HtmlReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for HtmlReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let contents = crate::formats::common::read_string_capped(reader)?;

        let mut book = Book::new();

        // Extract metadata from <head>.
        book.metadata = parser::extract_metadata(&contents);

        // Extract body content.
        let body = parser::extract_body(&contents);

        // Extract data URI images from the body and add as resources.
        let body = extract_data_uri_images(body, &mut book);

        // Split into chapters (now using the modified body with resource paths).
        let chapters = parser::split_into_chapters(&body);

        if chapters.is_empty() {
            // Fallback: treat entire body as one chapter.
            book.add_chapter(Chapter {
                title: Some("Main Content".into()),
                content: body.to_string(),
                id: Some("main".into()),
            });
        } else {
            for (i, (title, content)) in chapters.into_iter().enumerate() {
                book.add_chapter(Chapter {
                    title: title.map(|t| t.into_owned()),
                    content: content.to_string(),
                    id: Some(format!("chapter_{}", i)),
                });
            }
        }

        // Default title if none found.
        if book.metadata.title.is_none() {
            book.metadata.title = Some("Unknown HTML Document".into());
        }

        Ok(book)
    }
}

/// HTML format writer.
///
/// Generates a standalone HTML5 document from a `Book`.
/// Chapters are written as sections with heading separators.
/// Images are embedded as base64 data URIs.
#[derive(Default)]
pub struct HtmlWriter;

impl HtmlWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for HtmlWriter {
    fn write_book(&self, book: &Book, writer: &mut dyn Write) -> Result<()> {
        let html = book_to_html(book);
        writer.write_all(html.as_bytes())?;
        Ok(())
    }
}

/// Converts a `Book` to a standalone HTML5 document string.
fn book_to_html(book: &Book) -> String {
    let title = book.metadata.title.as_deref().unwrap_or("Untitled");
    let chapters = book.chapter_views();

    // Build body content.
    let mut body = String::with_capacity(4096);

    for (i, chapter) in chapters.iter().enumerate() {
        if i > 0 {
            body.push_str("<hr />\n");
        }

        if let Some(ch_title) = chapter.title {
            body.push_str("<h1>");
            push_escape_html(&mut body, ch_title);
            body.push_str("</h1>\n");
        }

        let content = match chapter.title {
            Some(ch_title) => strip_leading_heading(chapter.content, ch_title),
            None => chapter.content,
        };
        body.push_str(content);
        body.push('\n');
    }

    // Embed images as base64 data URIs in a style block.
    let resources = book.resources();
    if !resources.is_empty() {
        body.push_str("\n<!-- Embedded resources -->\n");
        for res in &resources {
            if res.media_type.starts_with("image/") {
                body.push_str("<img src=\"data:");
                body.push_str(res.media_type);
                body.push_str(";base64,");
                base64_simd::STANDARD.encode_append(res.data, &mut body);
                body.push_str("\" alt=\"");
                push_escape_html(&mut body, res.id);
                body.push_str("\" />\n");
            }
        }
    }

    parser::build_html_document(title, &book.metadata, &body)
}

/// Extracts embedded base64 data URI images from HTML content.
///
/// Scans for `<img` tags with `src="data:{media_type};base64,{data}"` URIs,
/// decodes each image, adds it as a book resource, and replaces the data URI
/// with the resource path (e.g., `images/extracted_img_0.png`).
///
/// Returns the modified HTML with data URIs replaced by resource paths.
fn extract_data_uri_images<'a>(html: &'a str, book: &mut Book) -> Cow<'a, str> {
    // Fast path: skip allocation entirely when no data URIs are present.
    if !html.contains("src=\"data:") {
        return Cow::Borrowed(html);
    }

    let mut result = String::with_capacity(html.len());
    let mut remaining = html;
    let mut img_index: usize = 0;

    while let Some(src_pos) = remaining.find("src=\"data:") {
        // Copy everything up to and including `src="`
        result.push_str(&remaining[..src_pos + 5]);
        // Advance past `src="`
        remaining = &remaining[src_pos + 5..];

        // `remaining` now starts with `data:{media_type};base64,{b64_data}"...`
        // Find the closing quote for the src attribute value.
        let Some(quote_end) = remaining.find('"') else {
            // No closing quote -- broken HTML, just copy the rest.
            break;
        };

        let data_uri = &remaining[..quote_end];

        // Parse: data:{media_type};base64,{b64_data}
        if let Some(replaced) = parse_and_extract_data_uri(data_uri, book, img_index) {
            result.push_str(&replaced);
            img_index += 1;
        } else {
            // Could not parse -- keep the original data URI unchanged.
            result.push_str(data_uri);
        }
        result.push('"');
        remaining = &remaining[quote_end + 1..];
    }

    // Copy any remaining content.
    result.push_str(remaining);
    Cow::Owned(result)
}

/// Parses a single `data:` URI value, decodes the base64 payload, adds it as
/// a resource to the book, and returns the replacement path string.
///
/// Returns `None` if the URI cannot be parsed or decoded.
fn parse_and_extract_data_uri(uri: &str, book: &mut Book, index: usize) -> Option<String> {
    // Expected format: data:{media_type};base64,{b64_data}
    let after_data = uri.strip_prefix("data:")?;
    let semicolon = after_data.find(';')?;
    let media_type = &after_data[..semicolon];

    let after_semi = &after_data[semicolon + 1..];
    let b64_data = after_semi.strip_prefix("base64,")?;

    // Decode the base64 payload.
    let decoded = base64_simd::STANDARD
        .decode_to_vec(b64_data.as_bytes())
        .ok()?;

    // Determine file extension from media type.
    let ext = match media_type {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        _ => "bin",
    };

    let id = format!("extracted_img_{}", index);
    let href = format!("images/extracted_img_{}.{}", index, ext);

    book.add_resource(id, &href, decoded, media_type);

    Some(href)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn html_reader_extracts_metadata() {
        let html = r#"<!DOCTYPE html>
<html>
<head>
<title>Test Book</title>
<meta name="author" content="Alice">
<meta name="language" content="en">
</head>
<body>
<p>Hello world</p>
</body>
</html>"#;

        let mut cursor = Cursor::new(html.as_bytes());
        let book = HtmlReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Test Book"));
        assert_eq!(book.metadata.authors, vec!["Alice"]);
        assert_eq!(book.metadata.language.as_deref(), Some("en"));
    }

    #[test]
    fn html_reader_splits_chapters() {
        let html = r#"<html><head><title>T</title></head><body>
<h1>Chapter 1</h1><p>Content one</p>
<h1>Chapter 2</h1><p>Content two</p>
</body></html>"#;

        let mut cursor = Cursor::new(html.as_bytes());
        let book = HtmlReader::new().read_book(&mut cursor).unwrap();

        let chapters = book.chapters();
        assert_eq!(chapters.len(), 2);
        assert!(chapters[0].content.contains("Content one"));
        assert!(chapters[1].content.contains("Content two"));
    }

    #[test]
    fn html_reader_handles_fragment() {
        let html = "<p>Just a paragraph</p>";
        let mut cursor = Cursor::new(html.as_bytes());
        let book = HtmlReader::new().read_book(&mut cursor).unwrap();

        assert!(!book.chapters().is_empty());
        assert!(book.chapters()[0].content.contains("Just a paragraph"));
    }

    #[test]
    fn html_writer_produces_valid_html() {
        let mut book = Book::new();
        book.metadata.title = Some("My Book".into());
        book.metadata.authors.push("Bob".into());
        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello world</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        HtmlWriter::new().write_book(&book, &mut output).unwrap();
        let html = String::from_utf8(output).unwrap();

        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<title>My Book</title>"));
        assert!(html.contains("name=\"author\""));
        assert!(html.contains("content=\"Bob\""));
        assert!(html.contains("<h1>Chapter 1</h1>"));
        assert!(html.contains("Hello world"));
    }

    #[test]
    fn html_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("Round Trip".into());
        book.metadata.authors.push("Author".into());
        book.add_chapter(Chapter {
            title: Some("Ch 1".into()),
            content: "<p>Content here</p>".into(),
            id: Some("ch1".into()),
        });

        // Write
        let mut output = Vec::new();
        HtmlWriter::new().write_book(&book, &mut output).unwrap();

        // Read back
        let mut cursor = Cursor::new(output);
        let decoded = HtmlReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("Round Trip"));
        assert!(!decoded.chapters().is_empty());
    }

    #[test]
    fn html_writer_embeds_images() {
        let mut book = Book::new();
        book.metadata.title = Some("Image Test".into());
        book.add_chapter(Chapter {
            title: None,
            content: "<p>text</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_resource("img1", "cover.png", vec![0x89, 0x50], "image/png");

        let mut output = Vec::new();
        HtmlWriter::new().write_book(&book, &mut output).unwrap();
        let html = String::from_utf8(output).unwrap();

        assert!(html.contains("data:image/png;base64,"));
    }

    #[test]
    fn html_writer_no_duplicate_heading() {
        let mut book = Book::new();
        book.metadata.title = Some("Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch 1".into()),
            content: "<h1>Ch 1</h1><p>Body text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        HtmlWriter::new().write_book(&book, &mut output).unwrap();
        let html = String::from_utf8(output).unwrap();

        // The <h1>Ch 1</h1> heading should appear exactly once.
        let count = html.matches("<h1>Ch 1</h1>").count();
        assert_eq!(
            count, 1,
            "Expected one <h1>Ch 1</h1>, found {count} in: {html}"
        );
        assert!(html.contains("Body text"));
    }

    #[test]
    fn extract_single_data_uri_image() {
        let png_bytes = vec![0x89, 0x50, 0x4E, 0x47];
        let b64 = base64_simd::STANDARD.encode_to_string(&png_bytes);
        let html = format!(
            r#"<p>Before</p><img src="data:image/png;base64,{}" alt="test" /><p>After</p>"#,
            b64
        );

        let mut book = Book::new();
        let result = extract_data_uri_images(&html, &mut book);

        // The data URI should be replaced with a resource path.
        assert!(
            result.contains(r#"src="images/extracted_img_0.png""#),
            "Data URI should be replaced with resource path. Got: {}",
            result
        );
        assert!(
            !result.contains("base64,"),
            "No base64 data should remain in the HTML. Got: {}",
            result
        );
        assert!(result.contains("Before"));
        assert!(result.contains("After"));

        // A resource should have been added.
        let resources = book.resources();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].id, "extracted_img_0");
        assert_eq!(resources[0].href, "images/extracted_img_0.png");
        assert_eq!(resources[0].media_type, "image/png");
        assert_eq!(resources[0].data, &png_bytes);
    }

    #[test]
    fn extract_multiple_data_uri_images() {
        let png_bytes = vec![0x89, 0x50];
        let jpg_bytes = vec![0xFF, 0xD8];
        let png_b64 = base64_simd::STANDARD.encode_to_string(&png_bytes);
        let jpg_b64 = base64_simd::STANDARD.encode_to_string(&jpg_bytes);
        let html = format!(
            r#"<img src="data:image/png;base64,{}" /><img src="data:image/jpeg;base64,{}" />"#,
            png_b64, jpg_b64
        );

        let mut book = Book::new();
        let result = extract_data_uri_images(&html, &mut book);

        assert!(result.contains(r#"src="images/extracted_img_0.png""#));
        assert!(result.contains(r#"src="images/extracted_img_1.jpg""#));
        assert!(!result.contains("base64,"));

        let resources = book.resources();
        assert_eq!(resources.len(), 2);

        let png_res = resources.iter().find(|r| r.media_type == "image/png");
        let jpg_res = resources.iter().find(|r| r.media_type == "image/jpeg");
        assert!(png_res.is_some());
        assert!(jpg_res.is_some());
        assert_eq!(png_res.unwrap().data, &png_bytes);
        assert_eq!(jpg_res.unwrap().data, &jpg_bytes);
    }

    #[test]
    fn extract_data_uri_no_images() {
        let html = "<p>No images here</p><img src=\"photo.jpg\" />";
        let mut book = Book::new();
        let result = extract_data_uri_images(html, &mut book);

        assert_eq!(result, html, "HTML without data URIs should be unchanged");
        assert!(book.resources().is_empty());
    }

    #[test]
    fn extract_data_uri_correct_media_types() {
        let data = vec![0x01, 0x02];
        let b64 = base64_simd::STANDARD.encode_to_string(&data);

        for (media, ext) in [
            ("image/png", "png"),
            ("image/jpeg", "jpg"),
            ("image/gif", "gif"),
            ("image/svg+xml", "svg"),
        ] {
            let html = format!(r#"<img src="data:{};base64,{}" />"#, media, b64);
            let mut book = Book::new();
            let result = extract_data_uri_images(&html, &mut book);

            let expected_href = format!("images/extracted_img_0.{}", ext);
            assert!(
                result.contains(&format!(r#"src="{}""#, expected_href)),
                "Media type {} should produce extension .{}. Got: {}",
                media,
                ext,
                result
            );

            let resources = book.resources();
            assert_eq!(resources.len(), 1);
            assert_eq!(resources[0].media_type, media);
            assert_eq!(resources[0].data, &data);
        }
    }

    #[test]
    fn html_round_trip_with_images() {
        let mut book = Book::new();
        book.metadata.title = Some("Image Round Trip".into());
        book.add_chapter(Chapter {
            title: Some("Ch 1".into()),
            content: "<p>Content with images</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_resource(
            "img1",
            "cover.png",
            vec![0x89, 0x50, 0x4E, 0x47],
            "image/png",
        );

        // Write to HTML (embeds images as data URIs).
        let mut output = Vec::new();
        HtmlWriter::new().write_book(&book, &mut output).unwrap();

        // Read back (should extract data URIs back into resources).
        let mut cursor = Cursor::new(output);
        let decoded = HtmlReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("Image Round Trip"));
        let resources = decoded.resources();
        assert!(
            !resources.is_empty(),
            "Round-tripped book should have image resources"
        );
        let img = resources.iter().find(|r| r.media_type == "image/png");
        assert!(img.is_some(), "Should have a PNG resource");
        assert_eq!(
            img.unwrap().data,
            &[0x89, 0x50, 0x4E, 0x47],
            "Image data should be preserved through round trip"
        );
    }
}
