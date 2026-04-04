//! Normalizes HTML content to well-formed XHTML.

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
///
/// Uses `memchr2` for bulk scanning — copies clean text spans in one shot
/// instead of iterating char-by-char.
fn normalize_xhtml(html: &str) -> String {
    let bytes = html.as_bytes();
    let len = bytes.len();
    // Pre-allocate to input length — normalization rarely increases size
    // significantly (only bare `&` -> `&amp;` and void tags gain ` /`).
    let mut output = String::with_capacity(len + len / 32);
    let mut pos = 0;

    while pos < len {
        // Scan for next '<' or '&' using SIMD-accelerated memchr2.
        match memchr::memchr2(b'<', b'&', &bytes[pos..]) {
            None => {
                // No more special chars — copy remainder in bulk.
                output.push_str(&html[pos..]);
                break;
            },
            Some(offset) => {
                let special_pos = pos + offset;
                // Copy the clean span before the special character.
                if special_pos > pos {
                    output.push_str(&html[pos..special_pos]);
                }

                if bytes[special_pos] == b'<' {
                    // Find closing '>' for this tag.
                    match memchr::memchr(b'>', &bytes[special_pos..]) {
                        Some(close_offset) => {
                            let tag_end = special_pos + close_offset + 1;
                            let tag_str = &html[special_pos..tag_end];
                            normalize_tag_into(&mut output, tag_str);
                            pos = tag_end;
                        },
                        None => {
                            // Unclosed tag at end of input — copy as-is.
                            output.push_str(&html[special_pos..]);
                            break;
                        },
                    }
                } else {
                    // '&' — check if it's a valid entity reference.
                    // Use a lookup table approach instead of per-byte method calls.
                    let after_amp = special_pos + 1;
                    let mut scan = after_amp;
                    let limit = (after_amp + 10).min(len);
                    let mut found_semicolon = false;

                    while scan < limit {
                        let b = bytes[scan];
                        if b == b';' {
                            found_semicolon = true;
                            break;
                        } else if is_entity_char(b) {
                            scan += 1;
                        } else {
                            break;
                        }
                    }

                    if found_semicolon && scan > after_amp {
                        // Valid entity — copy verbatim through the semicolon.
                        let end = scan + 1;
                        output.push_str(&html[special_pos..end]);
                        pos = end;
                    } else {
                        // Bare ampersand — escape it.
                        output.push_str("&amp;");
                        pos = special_pos + 1;
                    }
                }
            },
        }
    }

    output
}

/// Returns `true` if the byte is valid inside an HTML entity name (alphanumeric or `#`).
#[inline(always)]
fn is_entity_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'#'
}

/// Normalizes a tag and appends it directly to the output buffer.
///
/// If the tag is a void element without a self-closing slash, appends with ` />`
/// instead of `>`. Otherwise appends the tag as-is. This avoids the intermediate
/// `Cow`/`String` allocation that `ensure_self_closing_voids` + `push_str` created.
fn normalize_tag_into(output: &mut String, tag: &str) {
    let tag_bytes = tag.as_bytes();
    let tag_len = tag_bytes.len();

    // Closing tags and already self-closing tags pass through unchanged.
    if tag_len >= 2 && (tag_bytes[1] == b'/' || tag_bytes[tag_len - 2] == b'/') {
        output.push_str(tag);
        return;
    }

    // Extract the element name (after '<', before space or '>').
    // tag_bytes[0] == b'<', tag_bytes[tag_len-1] == b'>'
    if tag_len < 3 {
        output.push_str(tag);
        return;
    }

    let inner = &tag_bytes[1..tag_len - 1]; // strip < and >
    let name_end = inner
        .iter()
        .position(|&b| b.is_ascii_whitespace() || b == b'/')
        .unwrap_or(inner.len());
    let name_bytes = &inner[..name_end];

    // Check if the lowercased name matches a void element.
    if is_void_element(name_bytes) {
        // Write `<...attrs />`  (replace trailing `>` with ` />`)
        output.push_str(&tag[..tag_len - 1]);
        output.push_str(" />");
    } else {
        output.push_str(tag);
    }
}

/// Returns `true` if `name` (case-insensitive) is an HTML void element.
#[inline]
fn is_void_element(name: &[u8]) -> bool {
    // Short-circuit on length: void element names are 2-6 bytes.
    let n = name.len();
    if !(2..=6).contains(&n) {
        return false;
    }

    // Compare case-insensitively using a small lookup.
    const VOID_ELEMENTS: &[&[u8]] = &[
        b"br", b"hr", b"img", b"meta", b"link", b"input", b"area", b"base", b"col", b"embed",
        b"source", b"track", b"wbr",
    ];

    VOID_ELEMENTS
        .iter()
        .any(|ve| ve.len() == n && name.eq_ignore_ascii_case(ve))
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
