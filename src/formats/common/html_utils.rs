use std::borrow::Cow;

/// Escapes text for safe embedding in HTML/XHTML content.
pub fn escape_html(text: &str) -> Cow<'_, str> {
    super::text_utils::escape_html(text)
}

/// Strips all HTML tags from content, returning plain text.
/// This is a simple implementation -- not a full HTML parser.
pub fn strip_tags(html: &str) -> Cow<'_, str> {
    super::text_utils::strip_tags(html)
}

/// Wraps text in a paragraph element.
pub fn wrap_paragraph(text: &str) -> String {
    format!("<p>{}</p>", text)
}

/// Wraps content in a minimal XHTML document shell.
pub fn wrap_xhtml(title: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>{}</title></head>
<body>
{}
</body>
</html>"#,
        escape_html(title),
        body
    )
}

/// Strips a leading `<h1>` or `<h2>` heading from HTML content if its text matches `title`.
///
/// Returns the remaining content (trimmed). If the leading heading does not match
/// the supplied title (case-insensitive, whitespace-normalised), the original
/// content is returned unchanged.
pub fn strip_leading_heading<'a>(content: &'a str, title: &str) -> &'a str {
    let bytes = content.as_bytes();
    let trimmed_start = super::text_utils::skip_whitespace(bytes);
    let rest = &bytes[trimmed_start..];

    // Must start with '<h1' or '<h2' (case-insensitive).
    if rest.len() < 4 || rest[0] != b'<' {
        return content;
    }
    if !(rest[1].eq_ignore_ascii_case(&b'h')
        && (rest[2] == b'1' || rest[2] == b'2')
        && (rest[3] == b'>' || rest[3] == b' ' || rest[3] == b'\t'))
    {
        return content;
    }

    let heading_level = rest[2]; // b'1' or b'2'

    // Build the closing tag to search for, e.g. "</h1>" or "</h2>".
    let close_tag: [u8; 5] = [b'<', b'/', b'h', heading_level, b'>'];

    // Find the closing tag (case-insensitive).
    let rest_str = &content[trimmed_start..];
    let close_pos = match super::text_utils::find_case_insensitive(rest_str.as_bytes(), &close_tag)
    {
        Some(pos) => pos,
        None => return content,
    };

    // Extract text between open and close tags, strip inner HTML to get plain text.
    let open_tag_end = match rest_str.find('>') {
        Some(pos) => pos + 1,
        None => return content,
    };
    let heading_html = &rest_str[open_tag_end..close_pos];
    let heading_text = strip_tags(heading_html);

    // Normalise whitespace for comparison: collapse runs of whitespace to a single space.
    let normalise = |s: &str| -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    };

    let normalised_heading = normalise(heading_text.as_ref());
    let normalised_title = normalise(title);

    if normalised_heading.eq_ignore_ascii_case(&normalised_title) {
        let after_close = trimmed_start + close_pos + close_tag.len();
        content[after_close..].trim_start()
    } else {
        content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_tags_removes_html() {
        assert_eq!(strip_tags("<p>Hello <b>world</b></p>"), "Hello world");
    }

    #[test]
    fn strip_tags_preserves_plain_text() {
        assert_eq!(strip_tags("no tags here"), "no tags here");
    }

    #[test]
    fn escape_html_handles_ampersand() {
        assert_eq!(escape_html("a & b"), "a &amp; b");
    }

    #[test]
    fn wrap_xhtml_produces_valid_structure() {
        let result = wrap_xhtml("Test", "<p>Content</p>");
        assert!(result.contains("<title>Test</title>"));
        assert!(result.contains("<p>Content</p>"));
        assert!(result.contains("xmlns=\"http://www.w3.org/1999/xhtml\""));
    }

    // -- strip_leading_heading -----------------------------------------------

    #[test]
    fn strip_leading_heading_h1_match() {
        let content = "<h1>Chapter 1</h1><p>Body text here</p>";
        let result = strip_leading_heading(content, "Chapter 1");
        assert_eq!(result, "<p>Body text here</p>");
    }

    #[test]
    fn strip_leading_heading_h2_match() {
        let content = "<h2>Section A</h2><p>Content follows</p>";
        let result = strip_leading_heading(content, "Section A");
        assert_eq!(result, "<p>Content follows</p>");
    }

    #[test]
    fn strip_leading_heading_no_match() {
        let content = "<h1>Different Title</h1><p>Body text</p>";
        let result = strip_leading_heading(content, "Chapter 1");
        assert_eq!(result, content);
    }

    #[test]
    fn strip_leading_heading_no_heading() {
        let content = "<p>Just a paragraph</p>";
        let result = strip_leading_heading(content, "Some Title");
        assert_eq!(result, content);
    }

    #[test]
    fn strip_leading_heading_case_insensitive() {
        let content = "<H1>chapter one</H1><p>Body</p>";
        let result = strip_leading_heading(content, "CHAPTER ONE");
        assert_eq!(result, "<p>Body</p>");
    }

    #[test]
    fn strip_leading_heading_whitespace_normalised() {
        let content = "<h1>  Chapter   One  </h1><p>Body</p>";
        let result = strip_leading_heading(content, "Chapter One");
        assert_eq!(result, "<p>Body</p>");
    }

    #[test]
    fn strip_leading_heading_with_leading_whitespace() {
        let content = "  \n  <h1>Title</h1><p>Text</p>";
        let result = strip_leading_heading(content, "Title");
        assert_eq!(result, "<p>Text</p>");
    }

    #[test]
    fn strip_leading_heading_with_inner_tags() {
        let content = "<h1><b>Bold Title</b></h1><p>Content</p>";
        let result = strip_leading_heading(content, "Bold Title");
        assert_eq!(result, "<p>Content</p>");
    }
}
