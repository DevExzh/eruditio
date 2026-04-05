//! Extracts data URI images from HTML content into manifest resources.

use base64::Engine;

use crate::domain::Book;
use crate::domain::manifest::{ManifestData, ManifestItem};
use crate::domain::traits::Transform;
use crate::error::Result;

/// Extracts data URI images from chapter HTML content and adds them as manifest resources.
///
/// Scans all spine items for `<img src="data:...">` patterns, decodes the base64 data,
/// adds each image as a manifest resource, and replaces the `src` attribute with a
/// relative path.
pub struct DataUriExtractor;

impl Transform for DataUriExtractor {
    fn name(&self) -> &str {
        "data_uri_extractor"
    }

    fn apply(&self, book: Book) -> Result<Book> {
        let mut result = book;
        let mut image_counter = 0u32;

        // Collect spine item IDs first to avoid borrow issues.
        let spine_ids: Vec<String> = result
            .spine
            .iter()
            .map(|s| s.manifest_id.clone())
            .collect();

        for manifest_id in &spine_ids {
            if let Some(item) = result.manifest.get_mut(manifest_id) {
                if let Some(text) = item.data.as_text() {
                    let text = text.to_string(); // Clone to release borrow
                    let (new_content, extracted) = extract_data_uris(&text, &mut image_counter);

                    if !extracted.is_empty() {
                        // Update the chapter content.
                        item.data = ManifestData::Text(new_content);

                        // Add extracted images to manifest.
                        for img in extracted {
                            let manifest_item =
                                ManifestItem::new(&img.id, &img.href, &img.media_type)
                                    .with_data(img.data);
                            result.manifest.insert(manifest_item);
                        }
                    }
                }
            }
        }

        Ok(result)
    }
}

/// An image that was extracted from a data URI.
struct ExtractedImage {
    id: String,
    href: String,
    media_type: String,
    data: Vec<u8>,
}

/// Returns a file extension for the given MIME type.
fn mime_to_extension(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        _ => "bin",
    }
}

/// Scans HTML for `src="data:..."` (or single-quoted) attributes, decodes the base64 data,
/// and returns the modified HTML and a list of extracted images.
fn extract_data_uris(html: &str, counter: &mut u32) -> (String, Vec<ExtractedImage>) {
    let mut result = String::with_capacity(html.len());
    let mut images = Vec::new();
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut pos = 0;

    while pos < len {
        // Search for `src="data:` or `src='data:` (case-insensitive on `src`).
        if let Some(offset) = find_data_uri_src(&bytes[pos..]) {
            let attr_start = pos + offset;

            // Determine which quote character is used.
            // `offset` points to the `s` in `src`, so we need to find the quote.
            // The pattern is: src="data:  or  src='data:
            // Find the quote after `src=`.
            let eq_pos = match memchr::memchr(b'=', &bytes[attr_start..]) {
                Some(o) => attr_start + o,
                None => {
                    result.push_str(&html[pos..pos + offset + 1]);
                    pos = attr_start + 1;
                    continue;
                }
            };

            let quote_pos = eq_pos + 1;
            if quote_pos >= len {
                result.push_str(&html[pos..]);
                break;
            }
            let quote = bytes[quote_pos];
            if quote != b'"' && quote != b'\'' {
                // Not a quoted attribute, skip past it.
                result.push_str(&html[pos..quote_pos + 1]);
                pos = quote_pos + 1;
                continue;
            }

            // Find the closing quote.
            let data_start = quote_pos + 1; // Points right after the opening quote, i.e. at `d` of `data:`
            let closing_quote = match memchr::memchr(quote, &bytes[data_start..]) {
                Some(o) => data_start + o,
                None => {
                    // Malformed — no closing quote. Copy up to data_start and continue.
                    result.push_str(&html[pos..data_start]);
                    pos = data_start;
                    continue;
                }
            };

            // Extract the full data URI value between the quotes.
            let data_uri = &html[data_start..closing_quote];

            // Parse the data URI: data:[<mediatype>][;base64],<data>
            if let Some(parsed) = parse_data_uri(data_uri) {
                // Decode the base64 data.
                match base64::engine::general_purpose::STANDARD.decode(parsed.data) {
                    Ok(decoded) => {
                        let idx = *counter;
                        *counter += 1;
                        let ext = mime_to_extension(&parsed.mime);
                        let id = format!("extracted_img_{idx}");
                        let href = format!("images/extracted_{idx}.{ext}");

                        // Copy everything before the opening quote (inclusive of `src=`
                        // attribute name), then write the new src value.
                        // We want to replace: src="data:..."  with  src="images/extracted_N.ext"
                        // So copy everything up to and including the opening quote,
                        // then the new href, then the closing quote.
                        result.push_str(&html[pos..data_start]); // includes src=" (or src=')
                        result.push_str(&href);
                        // The closing quote will be picked up as part of the remaining text.
                        pos = closing_quote; // position at the closing quote (it will be copied next iteration)

                        images.push(ExtractedImage {
                            id,
                            href,
                            media_type: parsed.mime,
                            data: decoded,
                        });
                    }
                    Err(_) => {
                        // Malformed base64 — leave the original data URI intact.
                        result.push_str(&html[pos..closing_quote]);
                        pos = closing_quote;
                    }
                }
            } else {
                // Not a valid data URI (e.g., no `;base64,` marker) — leave intact.
                result.push_str(&html[pos..closing_quote]);
                pos = closing_quote;
            }
        } else {
            // No more data URIs found — copy the rest.
            result.push_str(&html[pos..]);
            break;
        }
    }

    (result, images)
}

/// Result of parsing a data URI.
struct ParsedDataUri<'a> {
    mime: String,
    data: &'a str,
}

/// Parses a data URI string (without the surrounding quotes).
/// Expected format: `data:<mime>;base64,<data>`
fn parse_data_uri(uri: &str) -> Option<ParsedDataUri<'_>> {
    let rest = uri.strip_prefix("data:")?;

    // Find `;base64,` separator.
    let base64_marker = ";base64,";
    let marker_pos = rest.find(base64_marker)?;

    let mime = rest[..marker_pos].to_string();
    let data = &rest[marker_pos + base64_marker.len()..];

    if mime.is_empty() || data.is_empty() {
        return None;
    }

    Some(ParsedDataUri { mime, data })
}

/// Searches a byte slice for the pattern `src="data:` or `src='data:` (case-insensitive `src`).
/// Returns the byte offset of the `s` in `src` if found.
fn find_data_uri_src(bytes: &[u8]) -> Option<usize> {
    let len = bytes.len();
    // We need at least `src="data:` = 10 bytes.
    if len < 10 {
        return None;
    }

    let mut i = 0;
    while i + 10 <= len {
        // Fast scan: find `s` or `S` using memchr2.
        let remaining = &bytes[i..];
        let offset = match memchr::memchr2(b's', b'S', remaining) {
            Some(o) => o,
            None => return None,
        };

        let start = i + offset;
        // Check if we have enough room for `src="data:`.
        if start + 10 > len {
            return None;
        }

        // Check `rc=` (case-insensitive).
        let b1 = bytes[start + 1];
        let b2 = bytes[start + 2];
        let b3 = bytes[start + 3];

        if (b1 == b'r' || b1 == b'R')
            && (b2 == b'c' || b2 == b'C')
            && b3 == b'='
        {
            let quote = bytes[start + 4];
            if (quote == b'"' || quote == b'\'')
                && bytes[start + 5] == b'd'
                && bytes[start + 6] == b'a'
                && bytes[start + 7] == b't'
                && bytes[start + 8] == b'a'
                && bytes[start + 9] == b':'
            {
                return Some(start);
            }
        }

        i = start + 1;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Book, Chapter};

    // 1x1 red pixel PNG encoded in base64.
    const TINY_PNG_B64: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==";

    fn decode_tiny_png() -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(TINY_PNG_B64)
            .unwrap()
    }

    #[test]
    fn basic_extraction() {
        let html = format!(
            r#"<html><body><img src="data:image/png;base64,{}" /></body></html>"#,
            TINY_PNG_B64
        );
        let mut counter = 0;
        let (result, images) = extract_data_uris(&html, &mut counter);

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].id, "extracted_img_0");
        assert_eq!(images[0].href, "images/extracted_0.png");
        assert_eq!(images[0].media_type, "image/png");
        assert_eq!(images[0].data, decode_tiny_png());
        assert!(result.contains(r#"src="images/extracted_0.png""#));
        assert!(!result.contains("data:image/png"));
    }

    #[test]
    fn multiple_images() {
        let html = format!(
            r#"<p><img src="data:image/png;base64,{}" /><img src="data:image/jpeg;base64,{}" /></p>"#,
            TINY_PNG_B64, TINY_PNG_B64
        );
        let mut counter = 0;
        let (result, images) = extract_data_uris(&html, &mut counter);

        assert_eq!(images.len(), 2);
        assert_eq!(images[0].id, "extracted_img_0");
        assert_eq!(images[0].href, "images/extracted_0.png");
        assert_eq!(images[1].id, "extracted_img_1");
        assert_eq!(images[1].href, "images/extracted_1.jpg");
        assert!(result.contains(r#"src="images/extracted_0.png""#));
        assert!(result.contains(r#"src="images/extracted_1.jpg""#));
        assert!(!result.contains("data:image"));
    }

    #[test]
    fn no_data_uris() {
        let html = r#"<html><body><img src="images/photo.jpg" /><p>No data URIs here</p></body></html>"#;
        let mut counter = 0;
        let (result, images) = extract_data_uris(html, &mut counter);

        assert!(images.is_empty());
        assert_eq!(result, html);
    }

    #[test]
    fn non_image_data_uri() {
        let html = format!(
            r#"<img src="data:text/plain;base64,{}" />"#,
            TINY_PNG_B64
        );
        let mut counter = 0;
        let (result, images) = extract_data_uris(&html, &mut counter);

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].media_type, "text/plain");
        assert_eq!(images[0].href, "images/extracted_0.bin");
        assert!(result.contains(r#"src="images/extracted_0.bin""#));
    }

    #[test]
    fn mixed_content() {
        let html = format!(
            r#"<html><body><img src="photo.jpg" /><img src="data:image/png;base64,{}" /><img src="other.gif" /></body></html>"#,
            TINY_PNG_B64
        );
        let mut counter = 0;
        let (result, images) = extract_data_uris(&html, &mut counter);

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].id, "extracted_img_0");
        // Regular src attributes should be untouched.
        assert!(result.contains(r#"src="photo.jpg""#));
        assert!(result.contains(r#"src="other.gif""#));
        assert!(result.contains(r#"src="images/extracted_0.png""#));
        assert!(!result.contains("data:image"));
    }

    #[test]
    fn single_quoted_attribute() {
        let html = format!(
            "<img src='data:image/gif;base64,{}' />",
            TINY_PNG_B64
        );
        let mut counter = 0;
        let (result, images) = extract_data_uris(&html, &mut counter);

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].media_type, "image/gif");
        assert_eq!(images[0].href, "images/extracted_0.gif");
        assert!(result.contains("src='images/extracted_0.gif'"));
    }

    #[test]
    fn transform_applies_to_book() {
        let mut book = Book::new();
        let chapter_html = format!(
            r#"<html><body><p>Hello</p><img src="data:image/png;base64,{}" /></body></html>"#,
            TINY_PNG_B64
        );
        book.add_chapter(&Chapter {
            title: Some("Ch 1".into()),
            content: chapter_html,
            id: Some("ch1".into()),
        });

        let extractor = DataUriExtractor;
        let result = extractor.apply(book).unwrap();

        // The chapter content should have the relative path instead of data URI.
        let chapters = result.chapters();
        assert!(chapters[0].content.contains(r#"src="images/extracted_0.png""#));
        assert!(!chapters[0].content.contains("data:image"));

        // The extracted image should be in the manifest.
        let img = result.manifest.get("extracted_img_0").unwrap();
        assert_eq!(img.href, "images/extracted_0.png");
        assert_eq!(img.media_type, "image/png");
        assert_eq!(img.data.as_bytes().unwrap(), &decode_tiny_png()[..]);
    }

    #[test]
    fn counter_persists_across_chapters() {
        let mut book = Book::new();
        let html1 = format!(
            r#"<p><img src="data:image/png;base64,{}" /></p>"#,
            TINY_PNG_B64
        );
        let html2 = format!(
            r#"<p><img src="data:image/jpeg;base64,{}" /></p>"#,
            TINY_PNG_B64
        );
        book.add_chapter(&Chapter {
            title: Some("Ch 1".into()),
            content: html1,
            id: Some("ch1".into()),
        });
        book.add_chapter(&Chapter {
            title: Some("Ch 2".into()),
            content: html2,
            id: Some("ch2".into()),
        });

        let extractor = DataUriExtractor;
        let result = extractor.apply(book).unwrap();

        assert!(result.manifest.get("extracted_img_0").is_some());
        assert!(result.manifest.get("extracted_img_1").is_some());

        let img1 = result.manifest.get("extracted_img_1").unwrap();
        assert_eq!(img1.href, "images/extracted_1.jpg");
    }

    #[test]
    fn malformed_base64_is_skipped() {
        // "!!!" is not valid base64 data.
        let html = r#"<img src="data:image/png;base64,!!!" />"#;
        let mut counter = 0;
        let (result, images) = extract_data_uris(html, &mut counter);

        assert!(images.is_empty());
        // Content should pass through without panicking.
        assert!(result.contains("data:image/png;base64"));
    }

    #[test]
    fn mime_extension_mapping() {
        assert_eq!(mime_to_extension("image/png"), "png");
        assert_eq!(mime_to_extension("image/jpeg"), "jpg");
        assert_eq!(mime_to_extension("image/jpg"), "jpg");
        assert_eq!(mime_to_extension("image/gif"), "gif");
        assert_eq!(mime_to_extension("image/svg+xml"), "svg");
        assert_eq!(mime_to_extension("image/webp"), "webp");
        assert_eq!(mime_to_extension("image/bmp"), "bmp");
        assert_eq!(mime_to_extension("application/octet-stream"), "bin");
    }

    #[test]
    fn parse_data_uri_valid() {
        let uri = format!("data:image/png;base64,{}", TINY_PNG_B64);
        let parsed = parse_data_uri(&uri).unwrap();
        assert_eq!(parsed.mime, "image/png");
        assert_eq!(parsed.data, TINY_PNG_B64);
    }

    #[test]
    fn parse_data_uri_no_base64_marker() {
        // Missing ;base64, marker.
        let result = parse_data_uri("data:image/png,rawdata");
        assert!(result.is_none());
    }

    #[test]
    fn parse_data_uri_empty_data() {
        let result = parse_data_uri("data:image/png;base64,");
        assert!(result.is_none());
    }

    #[test]
    fn parse_data_uri_empty_mime() {
        let result = parse_data_uri("data:;base64,abc");
        assert!(result.is_none());
    }
}
