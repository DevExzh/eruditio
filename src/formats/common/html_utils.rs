/// Escapes text for safe embedding in HTML/XHTML content.
pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Strips all HTML tags from content, returning plain text.
/// This is a simple implementation — not a full HTML parser.
pub fn strip_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }

    result
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_tags_removes_html() {
        assert_eq!(
            strip_tags("<p>Hello <b>world</b></p>"),
            "Hello world"
        );
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
}
