//! HTML parsing utilities for extracting metadata and content.

use std::borrow::Cow;

use crate::domain::metadata::Metadata;
use crate::formats::common::text_utils;
use crate::formats::common::text_utils::{find_ci, push_escape_xml};

/// Extracts metadata from HTML `<head>` section.
///
/// Parses `<title>`, `<meta name="author">`, `<meta name="description">`,
/// `<meta name="language">`, and `<meta http-equiv="Content-Language">`.
pub(crate) fn extract_metadata(html: &str) -> Metadata {
    let mut meta = Metadata {
        title: extract_tag_content(html, "title"),
        ..Metadata::default()
    };

    // Extract <meta> tags.
    let head_content = extract_between(html, "<head", "</head>");
    if let Some(head) = head_content {
        extract_meta_tags(head, &mut meta);
    }

    meta
}

/// Extracts the body content from an HTML document.
///
/// Returns the content between `<body>` and `</body>`, or the entire
/// string if no body tags are found (for HTML fragments).
pub(crate) fn extract_body(html: &str) -> &str {
    if let Some(body) = extract_between(html, "<body", "</body>") {
        body
    } else {
        // No body tags — treat the entire input as body content.
        // Strip any head/html tags that might be present.
        strip_outer_tags(html)
    }
}

/// Splits HTML body content into chapters at heading boundaries.
///
/// Splits on `<h1>` and `<h2>` tags. Each chunk becomes a chapter.
/// If no headings are found, returns the entire content as one chapter.
///
/// Uses memchr to find `<` positions and checks for heading tags inline,
/// avoiding the allocation + memcpy of a full-body lowercased copy.
pub(crate) fn split_into_chapters<'a>(body: &'a str) -> Vec<(Option<Cow<'a, str>>, &'a str)> {
    let mut chapters = Vec::new();

    // Find all h1/h2 positions.
    let mut split_points: Vec<(usize, Cow<'a, str>)> = Vec::new();

    let bytes = body.as_bytes();
    let len = bytes.len();

    // Single pass: scan for `<` using memchr, then check for heading tags
    // with inline byte comparisons. This avoids allocating a full-body
    // lowercased copy (saves len bytes of heap + memcpy + lowercase pass).
    let mut search_from = 0;
    while let Some(lt_offset) = memchr::memchr(b'<', &bytes[search_from..]) {
        let abs_pos = search_from + lt_offset;

        // Need at least 3 more bytes for "hN>"
        if abs_pos + 3 >= len {
            break;
        }

        let b1 = bytes[abs_pos + 1];
        if (b1 == b'h' || b1 == b'H')
            && (bytes[abs_pos + 2] == b'1' || bytes[abs_pos + 2] == b'2')
        {
            let b3 = bytes[abs_pos + 3];
            // Verify this is a real tag (followed by '>', whitespace, or '/')
            if b3 == b'>' || b3 == b' ' || b3 == b'\t' || b3 == b'\n' || b3 == b'\r' || b3 == b'/' {
                let heading_level = bytes[abs_pos + 2];

                // Find closing '>' of the opening tag.
                if let Some(gt_offset) = memchr::memchr(b'>', &bytes[abs_pos..]) {
                    let content_start = abs_pos + gt_offset + 1;

                    // Find matching closing tag </hN> using memchr.
                    if let Some(close_pos) = find_closing_heading(bytes, content_start, heading_level) {
                        let heading_cow =
                            text_utils::strip_tags(&body[content_start..close_pos]);
                        let trimmed = heading_cow.trim();
                        if !trimmed.is_empty() {
                            let title: Cow<'a, str> = match heading_cow {
                                Cow::Borrowed(s) => {
                                    Cow::Borrowed(s.trim())
                                }
                                Cow::Owned(s) => Cow::Owned(s.trim().to_string()),
                            };
                            split_points.push((abs_pos, title));
                        }
                    }
                }
            }
        }

        search_from = abs_pos + 1;
    }

    // Sort by position (needed because h1 and h2 are found interleaved).
    split_points.sort_by_key(|(pos, _)| *pos);

    if split_points.is_empty() {
        // No headings — single chapter.
        let trimmed = body.trim();
        if !trimmed.is_empty() {
            chapters.push((None, trimmed));
        }
        return chapters;
    }

    // Content before the first heading.
    let before = body[..split_points[0].0].trim();
    if !before.is_empty() {
        chapters.push((None, before));
    }

    // Each heading starts a new chapter.
    // Drain split_points to avoid cloning the Cow titles.
    let mut split_iter = split_points.into_iter().peekable();
    while let Some((start, title)) = split_iter.next() {
        let end = split_iter.peek().map_or(body.len(), |(pos, _)| *pos);
        let content = body[start..end].trim();
        if !content.is_empty() {
            chapters.push((Some(title), content));
        }
    }

    chapters
}

/// Finds the byte offset of a closing `</hN>` tag (case-insensitive) starting
/// from `from`, where `level` is `b'1'` or `b'2'`.
///
/// Returns the offset into `bytes` where the closing tag starts (i.e., the
/// position of `<` in `</hN>`), or `None` if not found.
fn find_closing_heading(bytes: &[u8], from: usize, level: u8) -> Option<usize> {
    let mut search_from = from;
    while let Some(lt_offset) = memchr::memchr(b'<', &bytes[search_from..]) {
        let pos = search_from + lt_offset;
        // Need at least 5 bytes: </hN>
        if pos + 5 > bytes.len() {
            break;
        }
        if bytes[pos + 1] == b'/'
            && (bytes[pos + 2] == b'h' || bytes[pos + 2] == b'H')
            && bytes[pos + 3] == level
            && (bytes[pos + 4] == b'>' || bytes[pos + 4] == b' ' || bytes[pos + 4] == b'\t')
        {
            return Some(pos);
        }
        search_from = pos + 1;
    }
    None
}

/// Generates a complete HTML5 document from title and body content.
pub(crate) fn build_html_document(title: &str, meta: &Metadata, body: &str) -> String {
    let mut html = String::with_capacity(body.len() + 512);

    html.push_str("<!DOCTYPE html>\n<html>\n<head>\n");
    html.push_str("<meta charset=\"UTF-8\">\n");
    html.push_str("<title>");
    push_escape_xml(&mut html, title);
    html.push_str("</title>\n");

    for author in &meta.authors {
        html.push_str("<meta name=\"author\" content=\"");
        push_escape_xml(&mut html, author);
        html.push_str("\">\n");
    }
    if let Some(ref desc) = meta.description {
        html.push_str("<meta name=\"description\" content=\"");
        push_escape_xml(&mut html, desc);
        html.push_str("\">\n");
    }
    if let Some(ref lang) = meta.language {
        html.push_str("<meta name=\"language\" content=\"");
        push_escape_xml(&mut html, lang);
        html.push_str("\">\n");
    }

    html.push_str("</head>\n<body>\n");
    html.push_str(body);
    html.push_str("\n</body>\n</html>\n");

    html
}

/// Extracts text content between opening and closing tags of the given element.
/// Uses stack buffers for tag patterns to avoid heap allocation.
fn extract_tag_content(html: &str, tag: &str) -> Option<String> {
    let bytes = html.as_bytes();
    let tag_bytes = tag.as_bytes();
    debug_assert!(tag_bytes.len() < 60, "tag name too long for stack buffer");

    // Build open pattern "<tag" on the stack (no heap allocation).
    let mut open_buf = [0u8; 64];
    open_buf[0] = b'<';
    open_buf[1..1 + tag_bytes.len()].copy_from_slice(tag_bytes);
    let open_len = 1 + tag_bytes.len();

    // Build close pattern "</tag>" on the stack.
    let mut close_buf = [0u8; 64];
    close_buf[0] = b'<';
    close_buf[1] = b'/';
    close_buf[2..2 + tag_bytes.len()].copy_from_slice(tag_bytes);
    close_buf[2 + tag_bytes.len()] = b'>';
    let close_len = 3 + tag_bytes.len();

    let start = find_ci(bytes, &open_buf[..open_len])?;
    let gt = html[start..].find('>')?;
    let content_start = start + gt + 1;
    let content_end =
        find_ci(&bytes[content_start..], &close_buf[..close_len])?
            + content_start;

    let text = html[content_start..content_end].trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

/// Extracts content between an opening tag (with possible attributes) and closing tag.
fn extract_between<'a>(html: &'a str, open_prefix: &str, close_tag: &str) -> Option<&'a str> {
    let bytes = html.as_bytes();
    let start = find_ci(bytes, open_prefix.as_bytes())?;
    let gt = html[start..].find('>')?;
    let content_start = start + gt + 1;
    let content_end =
        find_ci(&bytes[content_start..], close_tag.as_bytes())?
            + content_start;

    Some(&html[content_start..content_end])
}

/// Extracts metadata from `<meta>` tags within head content.
///
/// Uses memchr to find `<` positions and checks for `meta` inline,
/// avoiding an `ascii_lowercase_copy` allocation of the entire head.
fn extract_meta_tags(head: &str, meta: &mut Metadata) {
    let bytes = head.as_bytes();
    let len = bytes.len();
    let mut search_from = 0;

    while let Some(lt_offset) = memchr::memchr(b'<', &bytes[search_from..]) {
        let abs_pos = search_from + lt_offset;

        // Need at least 5 bytes: <meta
        if abs_pos + 5 > len {
            break;
        }

        // Check for <meta case-insensitively.
        let is_meta = (bytes[abs_pos + 1] | 0x20) == b'm'
            && (bytes[abs_pos + 2] | 0x20) == b'e'
            && (bytes[abs_pos + 3] | 0x20) == b't'
            && (bytes[abs_pos + 4] | 0x20) == b'a'
            && (abs_pos + 5 >= len || matches!(bytes[abs_pos + 5], b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/'));

        if !is_meta {
            search_from = abs_pos + 1;
            continue;
        }

        let tag_end = match head[abs_pos..].find('>') {
            Some(e) => abs_pos + e,
            None => break,
        };

        let tag = &head[abs_pos..=tag_end];

        if let Some(name) = extract_attribute(tag, "name")
            && let Some(content) = extract_attribute(tag, "content")
        {
            match name {
                "author" | "dc.creator" => {
                    meta.authors.push(content.to_string());
                },
                "description" | "dc.description" => {
                    meta.description = Some(content.to_string());
                },
                "language" | "dc.language" => {
                    meta.language = Some(content.to_string());
                },
                "publisher" | "dc.publisher" => {
                    meta.publisher = Some(content.to_string());
                },
                "keywords" | "dc.subject" => {
                    for kw in content.split(',') {
                        let trimmed = kw.trim();
                        if !trimmed.is_empty() {
                            meta.subjects.push(trimmed.to_string());
                        }
                    }
                },
                _ => {},
            }
        }

        // Also check http-equiv for Content-Language.
        if text_utils::contains_ascii_ci(tag, "http-equiv")
            && text_utils::contains_ascii_ci(tag, "content-language")
            && let Some(content) = extract_attribute(tag, "content")
        {
            meta.language = Some(content.to_string());
        }

        search_from = tag_end + 1;
    }
}

/// Extracts an attribute value from an HTML tag string using case-insensitive
/// attribute name matching. No allocation is performed for the search pattern.
fn extract_attribute<'t>(tag: &'t str, attr_name: &str) -> Option<&'t str> {
    let attr_bytes = attr_name.as_bytes();
    debug_assert!(
        attr_bytes.len() < 62,
        "attribute name too long for stack buffer"
    );
    let mut pattern = [0u8; 64];
    let pat_len = attr_bytes.len() + 2;
    pattern[..attr_bytes.len()].copy_from_slice(attr_bytes);
    pattern[attr_bytes.len()] = b'=';
    pattern[attr_bytes.len() + 1] = b'"';

    let start = find_ci(tag.as_bytes(), &pattern[..pat_len])?;
    let value_start = start + pat_len;
    let value_end = tag[value_start..].find('"')? + value_start;
    Some(&tag[value_start..value_end])
}

/// Strips outer HTML structure tags (html, head, body) leaving content.
fn strip_outer_tags(html: &str) -> &str {
    let bytes = html.as_bytes();

    // Find </head> case-insensitively — skip everything up to and including it.
    let start = find_ci(bytes, b"</head>")
        .map(|p| p + 7)
        .unwrap_or(0);
    let working = &html[start..];
    let working_bytes = working.as_bytes();

    // Find the earliest of </html> or </body> and truncate there.
    let end = [
        find_ci(working_bytes, b"</html>"),
        find_ci(working_bytes, b"</body>"),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(working.len());
    let result = &working[..end];

    // Strip opening <body...> if present.
    if let Some(pos) = find_ci(result.as_bytes(), b"<body")
        && let Some(gt) = result[pos..].find('>')
    {
        return result[pos + gt + 1..].trim();
    }

    result.trim()
}

/// Escapes text for safe use in HTML attributes and content.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_title_from_html() {
        let html = "<html><head><title>My Book</title></head><body></body></html>";
        let meta = extract_metadata(html);
        assert_eq!(meta.title.as_deref(), Some("My Book"));
    }

    #[test]
    fn extracts_meta_author() {
        let html =
            r#"<html><head><meta name="author" content="Jane Doe"></head><body></body></html>"#;
        let meta = extract_metadata(html);
        assert_eq!(meta.authors, vec!["Jane Doe"]);
    }

    #[test]
    fn extracts_meta_description() {
        let html = r#"<html><head><meta name="description" content="A great book"></head><body></body></html>"#;
        let meta = extract_metadata(html);
        assert_eq!(meta.description.as_deref(), Some("A great book"));
    }

    #[test]
    fn extracts_body_content() {
        let html = "<html><head><title>T</title></head><body><p>Hello</p></body></html>";
        let body = extract_body(html);
        assert_eq!(body, "<p>Hello</p>");
    }

    #[test]
    fn body_fallback_for_fragment() {
        let html = "<p>Just a paragraph</p>";
        let body = extract_body(html);
        assert!(body.contains("Just a paragraph"));
    }

    #[test]
    fn splits_chapters_on_h1() {
        let body = "<h1>Chapter 1</h1><p>Content 1</p><h1>Chapter 2</h1><p>Content 2</p>";
        let chapters = split_into_chapters(body);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].0.as_deref(), Some("Chapter 1"));
        assert_eq!(chapters[1].0.as_deref(), Some("Chapter 2"));
    }

    #[test]
    fn single_chapter_for_no_headings() {
        let body = "<p>Just some text</p>";
        let chapters = split_into_chapters(body);
        assert_eq!(chapters.len(), 1);
        assert!(chapters[0].0.is_none());
    }

    #[test]
    fn build_document_includes_metadata() {
        let mut meta = Metadata::default();
        meta.authors.push("Alice".into());
        meta.language = Some("en".into());

        let doc = build_html_document("Test", &meta, "<p>Body</p>");
        assert!(doc.contains("<title>Test</title>"));
        assert!(doc.contains("name=\"author\""));
        assert!(doc.contains("content=\"Alice\""));
        assert!(doc.contains("name=\"language\""));
    }

    #[test]
    fn extract_attribute_works() {
        let tag = r#"<meta name="author" content="Bob">"#;
        assert_eq!(extract_attribute(tag, "name"), Some("author".into()));
        assert_eq!(extract_attribute(tag, "content"), Some("Bob".into()));
    }

    #[test]
    fn extracts_keywords_as_subjects() {
        let html = r#"<html><head><meta name="keywords" content="fiction, adventure, fantasy"></head><body></body></html>"#;
        let meta = extract_metadata(html);
        assert_eq!(meta.subjects, vec!["fiction", "adventure", "fantasy"]);
    }

    #[test]
    fn extract_tag_content_case_insensitive() {
        let html = "<html><head><TITLE>My Book</TITLE></head></html>";
        let meta = extract_metadata(html);
        assert_eq!(meta.title.as_deref(), Some("My Book"));
    }

    #[test]
    fn extract_body_mixed_case_tags() {
        let html = "<HTML><HEAD><title>T</title></HEAD><BODY><p>Content</p></BODY></HTML>";
        let body = extract_body(html);
        assert_eq!(body, "<p>Content</p>");
    }

    #[test]
    fn strip_outer_tags_case_insensitive() {
        let html =
            "<HTML><HEAD><title>T</title></HEAD><BODY class=\"x\"><p>Content</p></BODY></HTML>";
        let result = strip_outer_tags(html);
        assert_eq!(result, "<p>Content</p>");
    }

    #[test]
    fn strip_outer_tags_mixed_case() {
        let html = "<Html><Head><title>T</title></Head><Body><p>Hello</p></Body></Html>";
        let result = strip_outer_tags(html);
        assert_eq!(result, "<p>Hello</p>");
    }

    #[test]
    fn extracts_meta_from_uppercase_tags() {
        let html = r#"<html><head><META NAME="author" CONTENT="Jane"></head><body></body></html>"#;
        let meta = extract_metadata(html);
        assert_eq!(meta.authors, vec!["Jane"]);
    }
}
