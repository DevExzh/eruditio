//! HTML parsing utilities for extracting metadata and content.

use crate::domain::metadata::Metadata;
use crate::formats::common::text_utils;
use crate::formats::common::text_utils::escape_xml as escape_html;

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
        extract_meta_tags(&head, &mut meta);
    }

    meta
}

/// Extracts the body content from an HTML document.
///
/// Returns the content between `<body>` and `</body>`, or the entire
/// string if no body tags are found (for HTML fragments).
pub(crate) fn extract_body(html: &str) -> String {
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
pub(crate) fn split_into_chapters(body: &str) -> Vec<(Option<String>, String)> {
    let mut chapters = Vec::new();

    // Find all h1/h2 positions.
    let mut split_points: Vec<(usize, String)> = Vec::new();

    for level in 1..=2u8 {
        let open_tag = format!("<h{}", level);
        let close_tag = format!("</h{}>", level);

        let mut search_from = 0;
        while let Some(pos) =
            text_utils::find_case_insensitive(&body.as_bytes()[search_from..], open_tag.as_bytes())
        {
            let abs_pos = search_from + pos;

            // Extract the heading text.
            let after_open = &body[abs_pos..];
            if let Some(gt) = after_open.find('>') {
                let content_start = abs_pos + gt + 1;
                if let Some(close_pos) = text_utils::find_case_insensitive(
                    &body.as_bytes()[content_start..],
                    close_tag.as_bytes(),
                ) {
                    let heading_text =
                        strip_html_tags(&body[content_start..content_start + close_pos])
                            .trim()
                            .to_string();
                    if !heading_text.is_empty() {
                        split_points.push((abs_pos, heading_text));
                    }
                }
            }

            search_from = abs_pos + open_tag.len();
        }
    }

    // Sort by position.
    split_points.sort_by_key(|(pos, _)| *pos);

    if split_points.is_empty() {
        // No headings — single chapter.
        let trimmed = body.trim();
        if !trimmed.is_empty() {
            chapters.push((None, trimmed.to_string()));
        }
        return chapters;
    }

    // Content before the first heading.
    let before = body[..split_points[0].0].trim();
    if !before.is_empty() {
        chapters.push((None, before.to_string()));
    }

    // Each heading starts a new chapter.
    for i in 0..split_points.len() {
        let (start, ref title) = split_points[i];
        let end = if i + 1 < split_points.len() {
            split_points[i + 1].0
        } else {
            body.len()
        };

        let content = body[start..end].trim().to_string();
        if !content.is_empty() {
            chapters.push((Some(title.clone()), content));
        }
    }

    chapters
}

/// Generates a complete HTML5 document from title and body content.
pub(crate) fn build_html_document(title: &str, meta: &Metadata, body: &str) -> String {
    let mut html = String::with_capacity(body.len() + 512);

    html.push_str("<!DOCTYPE html>\n<html>\n<head>\n");
    html.push_str("<meta charset=\"UTF-8\">\n");
    html.push_str(&format!("<title>{}</title>\n", escape_html(title)));

    for author in &meta.authors {
        html.push_str(&format!(
            "<meta name=\"author\" content=\"{}\">\n",
            escape_html(author)
        ));
    }
    if let Some(ref desc) = meta.description {
        html.push_str(&format!(
            "<meta name=\"description\" content=\"{}\">\n",
            escape_html(desc)
        ));
    }
    if let Some(ref lang) = meta.language {
        html.push_str(&format!(
            "<meta name=\"language\" content=\"{}\">\n",
            escape_html(lang)
        ));
    }

    html.push_str("</head>\n<body>\n");
    html.push_str(body);
    html.push_str("\n</body>\n</html>\n");

    html
}

/// Extracts text content between opening and closing tags of the given element.
fn extract_tag_content(html: &str, tag: &str) -> Option<String> {
    let bytes = html.as_bytes();
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);

    let start = text_utils::find_case_insensitive(bytes, open.as_bytes())?;
    let gt = html[start..].find('>')?;
    let content_start = start + gt + 1;
    let content_end =
        text_utils::find_case_insensitive(&bytes[content_start..], close.as_bytes())?
            + content_start;

    let text = html[content_start..content_end].trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

/// Extracts content between an opening tag (with possible attributes) and closing tag.
fn extract_between(html: &str, open_prefix: &str, close_tag: &str) -> Option<String> {
    let bytes = html.as_bytes();
    let start = text_utils::find_case_insensitive(bytes, open_prefix.as_bytes())?;
    let gt = html[start..].find('>')?;
    let content_start = start + gt + 1;
    let content_end =
        text_utils::find_case_insensitive(&bytes[content_start..], close_tag.as_bytes())?
            + content_start;

    Some(html[content_start..content_end].to_string())
}

/// Extracts metadata from `<meta>` tags within head content.
fn extract_meta_tags(head: &str, meta: &mut Metadata) {
    let bytes = head.as_bytes();
    let mut search_from = 0;

    while let Some(pos) =
        text_utils::find_case_insensitive(&bytes[search_from..], b"<meta")
    {
        let abs_pos = search_from + pos;
        let tag_end = match head[abs_pos..].find('>') {
            Some(e) => abs_pos + e,
            None => break,
        };

        let tag = &head[abs_pos..=tag_end];
        let tag_lower = tag.to_lowercase();

        if let Some(name) = extract_attribute(&tag_lower, "name")
            && let Some(content) = extract_attribute_ci(&tag_lower, tag, "content")
        {
            match name.as_str() {
                "author" | "dc.creator" => {
                    meta.authors.push(content);
                },
                "description" | "dc.description" => {
                    meta.description = Some(content);
                },
                "language" | "dc.language" => {
                    meta.language = Some(content);
                },
                "publisher" | "dc.publisher" => {
                    meta.publisher = Some(content);
                },
                "keywords" | "dc.subject" => {
                    for kw in content.split(',') {
                        let trimmed = kw.trim().to_string();
                        if !trimmed.is_empty() {
                            meta.subjects.push(trimmed);
                        }
                    }
                },
                _ => {},
            }
        }

        // Also check http-equiv for Content-Language.
        if tag_lower.contains("http-equiv")
            && tag_lower.contains("content-language")
            && let Some(content) = extract_attribute_ci(&tag_lower, tag, "content")
        {
            meta.language = Some(content);
        }

        search_from = tag_end + 1;
    }
}

/// Extracts an attribute value from an HTML tag string.
fn extract_attribute(tag: &str, attr_name: &str) -> Option<String> {
    let pattern = format!("{}=\"", attr_name);
    let start = tag.find(&pattern)?;
    let value_start = start + pattern.len();
    let value_end = tag[value_start..].find('"')? + value_start;
    Some(tag[value_start..value_end].to_string())
}

/// Finds the attribute position in `tag_lower` (lowercased) but extracts the
/// value from `tag_orig` (original case). Both strings must have identical
/// byte length (true for ASCII HTML tags).
fn extract_attribute_ci(tag_lower: &str, tag_orig: &str, attr_name: &str) -> Option<String> {
    let pattern = format!("{}=\"", attr_name);
    let start = tag_lower.find(&pattern)?;
    let value_start = start + pattern.len();
    let value_end = tag_orig[value_start..].find('"')? + value_start;
    Some(tag_orig[value_start..value_end].to_string())
}

/// Strips HTML tags from a string.
fn strip_html_tags(html: &str) -> String {
    crate::formats::common::text_utils::strip_tags(html)
}

/// Strips outer HTML structure tags (html, head, body) leaving content.
fn strip_outer_tags(html: &str) -> String {
    let bytes = html.as_bytes();

    // Find </head> case-insensitively — skip everything up to and including it.
    let start = text_utils::find_case_insensitive(bytes, b"</head>")
        .map(|p| p + 7)
        .unwrap_or(0);
    let working = &html[start..];
    let working_bytes = working.as_bytes();

    // Find the earliest of </html> or </body> and truncate there.
    let end = [
        text_utils::find_case_insensitive(working_bytes, b"</html>"),
        text_utils::find_case_insensitive(working_bytes, b"</body>"),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(working.len());
    let result = &working[..end];

    // Strip opening <body...> if present.
    if let Some(pos) = text_utils::find_case_insensitive(result.as_bytes(), b"<body")
        && let Some(gt) = result[pos..].find('>')
    {
        return result[pos + gt + 1..].trim().to_string();
    }

    result.trim().to_string()
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
        let html = "<HTML><HEAD><title>T</title></HEAD><BODY class=\"x\"><p>Content</p></BODY></HTML>";
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
