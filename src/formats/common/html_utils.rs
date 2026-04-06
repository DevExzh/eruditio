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

/// Decodes the most common HTML/XML character entities (named and numeric).
pub fn unescape_basic_entities(text: &str) -> Cow<'_, str> {
    super::text_utils::unescape_basic_entities(text)
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
///
/// Handles full XHTML documents: if the content starts with `<?xml`, `<!DOCTYPE`,
/// or `<html`, the function locates `<body>` first and looks for the heading there.
pub fn strip_leading_heading<'a>(content: &'a str, title: &str) -> &'a str {
    let bytes = content.as_bytes();
    let trimmed_start = super::text_utils::skip_whitespace(bytes);

    // Determine where to look for the heading. If the content is a full XHTML
    // document, skip past `<body...>` first.
    let body_start = find_body_content_start(content, trimmed_start).unwrap_or(trimmed_start);
    let search_start = body_start + super::text_utils::skip_whitespace(&bytes[body_start..]);
    let rest = &bytes[search_start..];

    // Must start with '<h1' through '<h6' (case-insensitive).
    if rest.len() < 4 || rest[0] != b'<' {
        return content;
    }
    if !(rest[1].eq_ignore_ascii_case(&b'h')
        && rest[2].is_ascii_digit()
        && rest[2] >= b'1'
        && rest[2] <= b'6'
        && (rest[3] == b'>'
            || rest[3] == b' '
            || rest[3] == b'\t'
            || rest[3] == b'\n'
            || rest[3] == b'\r'))
    {
        return content;
    }

    let heading_level = rest[2]; // b'1' through b'6'

    // Build the closing tag to search for, e.g. "</h1>" through "</h6>".
    let close_tag: [u8; 5] = [b'<', b'/', b'h', heading_level, b'>'];

    // Find the closing tag (case-insensitive).
    let rest_str = &content[search_start..];
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
    let normalise = |s: &str| -> String { s.split_whitespace().collect::<Vec<_>>().join(" ") };

    let normalised_heading = normalise(heading_text.as_ref());
    let normalised_title = normalise(title);

    if normalised_heading.eq_ignore_ascii_case(&normalised_title)
        || normalised_heading
            .to_ascii_lowercase()
            .starts_with(&normalised_title.to_ascii_lowercase())
    {
        let after_close = search_start + close_pos + close_tag.len();
        let result = content[after_close..].trim_start();
        // If we're in a full XHTML document, also trim the closing </body>...</html>.
        if body_start > trimmed_start
            && let Some(pos) =
                super::text_utils::find_case_insensitive(result.as_bytes(), b"</body>")
        {
            return result[..pos].trim_end();
        }
        result
    } else {
        content
    }
}

/// If `content` contains a `<body...>` tag, returns the byte offset just after
/// the closing `>`. Returns `None` if no `<body` is found.
fn find_body_content_start(content: &str, from: usize) -> Option<usize> {
    let bytes = &content.as_bytes()[from..];
    // Only bother looking for <body if content looks like a full document.
    if bytes.len() < 6 {
        return None;
    }
    let first = bytes[0];
    // Quick check: full XHTML docs start with '<' followed by '?', '!', or 'h'/'H'.
    if first != b'<' {
        return None;
    }
    let second = bytes[1];
    if !(second == b'?' || second == b'!' || second.eq_ignore_ascii_case(&b'h')) {
        return None;
    }

    let pos = super::text_utils::find_case_insensitive(bytes, b"<body")?;
    let abs_pos = from + pos;
    let gt = content[abs_pos..].find('>')?;
    Some(abs_pos + gt + 1)
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

    #[test]
    fn strip_leading_heading_full_xhtml_document() {
        let content = r#"<?xml version='1.0' encoding='utf-8'?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>My Title</title></head>
<body>
  <h1 class="center">My Title</h1>
  <p>Body text</p>
</body>
</html>"#;
        let result = strip_leading_heading(content, "My Title");
        assert!(
            !result.contains("<h1"),
            "heading should be stripped, got: {result}"
        );
        assert!(result.contains("<p>Body text</p>"));
        assert!(
            !result.contains("</body>"),
            "trailing </body> should be trimmed, got: {result}"
        );
        assert!(
            !result.contains("</html>"),
            "trailing </html> should be trimmed, got: {result}"
        );
    }

    #[test]
    fn strip_leading_heading_full_xhtml_no_match() {
        let content = r#"<?xml version='1.0'?>
<html><head><title>T</title></head>
<body><h1>Different</h1><p>Text</p></body></html>"#;
        let result = strip_leading_heading(content, "Not This");
        assert_eq!(result, content);
    }

    #[test]
    fn strip_leading_heading_with_br_in_heading() {
        // The heading contains a <br/> which strip_tags now converts to a space.
        let content = "<h1>CHAPTER I.<br/>Down the Rabbit-Hole</h1><p>Body</p>";
        let result = strip_leading_heading(content, "CHAPTER I. Down the Rabbit-Hole");
        assert_eq!(result, "<p>Body</p>");
    }

    #[test]
    fn strip_leading_heading_title_is_prefix_of_heading() {
        // TOC title "CHAPTER I." is a prefix of the full heading text.
        let content = "<h2>CHAPTER I. Down the Rabbit-Hole</h2><p>Alice was beginning</p>";
        let result = strip_leading_heading(content, "CHAPTER I.");
        assert_eq!(result, "<p>Alice was beginning</p>");
    }

    #[test]
    fn strip_leading_heading_h3() {
        let content = "<h3>Section</h3><p>Content here</p>";
        let result = strip_leading_heading(content, "Section");
        assert_eq!(result, "<p>Content here</p>");
    }

    #[test]
    fn strip_leading_heading_h6() {
        let content = "<h6>Deep Heading</h6><p>Body</p>";
        let result = strip_leading_heading(content, "Deep Heading");
        assert_eq!(result, "<p>Body</p>");
    }
}
