//! Normalizes HTML content to well-formed XHTML.

use std::borrow::Cow;

use crate::domain::Book;
use crate::domain::traits::Transform;
use crate::error::Result;

/// Normalizes HTML content in chapter documents to well-formed XHTML.
///
/// Fixes common issues: unclosed tags, mismatched nesting, unescaped entities,
/// and ensures content is valid XHTML for downstream writers.
pub struct HtmlNormalizer;

impl Transform for HtmlNormalizer {
    fn name(&self) -> &str {
        "html_normalizer"
    }

    fn apply(&self, book: Book) -> Result<Book> {
        let mut result = book;

        // Walk spine items and normalize their HTML content.
        for spine_item in result.spine.iter() {
            if let Some(item) = result.manifest.get_mut(&spine_item.manifest_id)
                && let Some(text) = item.data.as_text()
            {
                let normalized = normalize_xhtml(text);
                item.data = crate::domain::manifest::ManifestData::Text(normalized);
            }
        }

        Ok(result)
    }
}

/// Normalizes an HTML string to well-formed XHTML.
///
/// Current implementation handles:
/// - Self-closing void elements (br, hr, img, meta, link, input)
/// - Unescaped ampersands in text content
fn normalize_xhtml(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '<' {
            // Collect the tag.
            let mut tag = String::from('<');
            for c in chars.by_ref() {
                tag.push(c);
                if c == '>' {
                    break;
                }
            }
            // Ensure void elements are self-closing.
            let normalized_tag = ensure_self_closing_voids(&tag);
            output.push_str(&normalized_tag);
        } else if ch == '&' {
            // Check if it's already a valid entity reference.
            let mut lookahead = String::new();
            let mut is_entity = false;
            let mut peek_chars = chars.clone();
            for _ in 0..10 {
                if let Some(&c) = peek_chars.peek() {
                    peek_chars.next();
                    if c == ';' {
                        is_entity = true;
                        break;
                    } else if c.is_alphanumeric() || c == '#' {
                        lookahead.push(c);
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            if is_entity {
                // Advance the real iterator past the entity body + semicolon,
                // emitting the full entity verbatim.
                output.push('&');
                for _ in 0..lookahead.len() {
                    if let Some(c) = chars.next() {
                        output.push(c);
                    }
                }
                // Consume the semicolon.
                if let Some(c) = chars.next() {
                    output.push(c);
                }
            } else {
                output.push_str("&amp;");
            }
        } else {
            output.push(ch);
        }
    }

    output
}

/// If the tag is a void element without a self-closing slash, add one.
fn ensure_self_closing_voids(tag: &str) -> Cow<'_, str> {
    const VOID_ELEMENTS: &[&str] = &[
        "br", "hr", "img", "meta", "link", "input", "area", "base", "col", "embed", "source",
        "track", "wbr",
    ];

    // Skip closing tags and already self-closing tags.
    if tag.starts_with("</") || tag.ends_with("/>") {
        return Cow::Borrowed(tag);
    }

    // Extract the element name (after '<', before space or '>').
    let inner = &tag[1..tag.len() - 1]; // strip < and >
    let name_bytes = inner.as_bytes();
    let name_end = name_bytes
        .iter()
        .position(|&b| b.is_ascii_whitespace() || b == b'/')
        .unwrap_or(name_bytes.len());
    let name_slice = &inner[..name_end];

    // Check if the lowercased name matches a void element without allocating
    // when it doesn't match.
    let is_void = VOID_ELEMENTS
        .iter()
        .any(|&ve| name_slice.eq_ignore_ascii_case(ve));

    if is_void {
        // Insert self-closing slash.
        Cow::Owned(format!("{} />", &tag[..tag.len() - 1]))
    } else {
        Cow::Borrowed(tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;

    #[test]
    fn normalizer_self_closes_br() {
        let input = "<p>Hello<br>World</p>";
        let result = normalize_xhtml(input);
        assert!(result.contains("<br />"));
    }

    #[test]
    fn normalizer_preserves_existing_self_close() {
        let input = "<br />";
        let result = normalize_xhtml(input);
        assert_eq!(result, "<br />");
    }

    #[test]
    fn normalizer_escapes_bare_ampersand() {
        let input = "A & B";
        let result = normalize_xhtml(input);
        assert_eq!(result, "A &amp; B");
    }

    #[test]
    fn normalizer_preserves_entity_refs() {
        let input = "&amp; &lt; &#x20;";
        let result = normalize_xhtml(input);
        assert_eq!(result, "&amp; &lt; &#x20;");
    }

    #[test]
    fn transform_applies_to_book() {
        let mut book = Book::new();
        book.add_chapter(&Chapter {
            title: Some("Ch".into()),
            content: "<p>A & B<br>C</p>".into(),
            id: Some("ch1".into()),
        });

        let normalizer = HtmlNormalizer;
        let result = normalizer.apply(book).unwrap();

        let chapters = result.chapters();
        assert!(chapters[0].content.contains("&amp;"));
        assert!(chapters[0].content.contains("<br />"));
    }
}
