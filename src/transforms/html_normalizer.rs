//! Normalizes HTML content to well-formed XHTML.

use std::borrow::Cow;

use crate::domain::Book;

/// Appends a `&str` slice to a `String` without re-validating UTF-8.
#[inline(always)]
fn push_str_unchecked(out: &mut String, s: &str) {
    // SAFETY: `s` is `&str`, so its bytes are guaranteed valid UTF-8.
    unsafe { out.as_mut_vec().extend_from_slice(s.as_bytes()) }
}
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
                // Only replace if normalization actually changed something,
                // avoiding a needless String clone for well-formed XHTML.
                if let Cow::Owned(s) = normalized {
                    item.data = crate::domain::manifest::ManifestData::Text(s);
                }
            }
        }

        Ok(result)
    }
}

/// Normalizes an HTML string to well-formed XHTML.
///
/// Current implementation handles:
/// - Stripping `<style>` and `<script>` elements (including their content)
/// - Self-closing void elements (br, hr, img, meta, link, input, etc.)
/// - Unescaped ampersands in text content
///
/// Returns `Cow::Borrowed` when the input is already well-formed, avoiding
/// allocation entirely. Uses `memchr2` for bulk scanning -- copies clean text
/// spans in one shot instead of iterating char-by-char.
///
/// Deferred-allocation strategy: the output buffer is only created when the
/// first modification is actually needed. For well-formed XHTML (the common
/// case in modern EPUBs), normalization is a no-op and we return
/// `Cow::Borrowed` with zero allocations.
fn normalize_xhtml(html: &str) -> Cow<'_, str> {
    let bytes = html.as_bytes();
    let len = bytes.len();

    // Fast path: scan the entire input for `<` or `&`. If neither is present,
    // there is nothing to normalize.
    if memchr::memchr2(b'<', b'&', bytes).is_none() {
        return Cow::Borrowed(html);
    }

    // `output` is None until the first modification is needed.
    // `copy_start` tracks the beginning of the next segment of `html` that
    // hasn't been flushed to `output` yet.
    let mut output: Option<String> = None;
    let mut copy_start: usize = 0;
    let mut pos = 0;

    while pos < len {
        // Scan for next '<' or '&' using SIMD-accelerated memchr2.
        match memchr::memchr2(b'<', b'&', &bytes[pos..]) {
            None => break,
            Some(offset) => {
                let special_pos = pos + offset;

                if bytes[special_pos] == b'<' {
                    // Check if this is a <style or <script opening tag whose
                    // content should be stripped entirely.
                    if let Some(block_end) = try_skip_style_or_script(bytes, special_pos) {
                        let out =
                            output.get_or_insert_with(|| String::with_capacity(len + len / 32));
                        push_str_unchecked(out, &html[copy_start..special_pos]);
                        copy_start = block_end;
                        pos = block_end;
                        continue;
                    }

                    // Find closing '>' for this tag.
                    match memchr::memchr(b'>', &bytes[special_pos..]) {
                        Some(close_offset) => {
                            let tag_end = special_pos + close_offset + 1;
                            let tag_str = &html[special_pos..tag_end];

                            if tag_needs_normalization(tag_str) {
                                // Lazily allocate the output buffer.
                                let out = output
                                    .get_or_insert_with(|| String::with_capacity(len + len / 32));
                                // Flush everything from copy_start up to the tag.
                                push_str_unchecked(out, &html[copy_start..special_pos]);
                                normalize_tag_into(out, tag_str);
                                copy_start = tag_end;
                            }
                            pos = tag_end;
                        },
                        None => {
                            // Unclosed tag at end of input -- nothing to change.
                            break;
                        },
                    }
                } else {
                    // '&' -- check if it's a valid entity reference.
                    let after_amp = special_pos + 1;
                    let mut scan = after_amp;
                    let limit = (after_amp + 10).min(len);
                    let mut found_semicolon = false;

                    while scan < limit {
                        let b = bytes[scan];
                        if b == b';' {
                            found_semicolon = true;
                            break;
                        } else if is_entity_char_fast(b) {
                            scan += 1;
                        } else {
                            break;
                        }
                    }

                    if found_semicolon && scan > after_amp {
                        // Valid entity -- skip past it, no changes needed.
                        pos = scan + 1;
                    } else {
                        // Bare ampersand -- escape it. Lazily allocate.
                        let out =
                            output.get_or_insert_with(|| String::with_capacity(len + len / 32));
                        // Flush everything from copy_start up to the ampersand.
                        push_str_unchecked(out, &html[copy_start..special_pos]);
                        out.push_str("&amp;");
                        copy_start = special_pos + 1;
                        pos = special_pos + 1;
                    }
                }
            },
        }
    }

    match output {
        Some(mut s) => {
            // Flush any remaining uncopied tail.
            if copy_start < len {
                push_str_unchecked(&mut s, &html[copy_start..]);
            }
            Cow::Owned(s)
        },
        None => Cow::Borrowed(html),
    }
}

/// If `bytes[tag_start..]` begins with `<style` or `<script` (case-insensitive),
/// followed by whitespace, `>`, or `/`, returns the byte position just past the
/// matching closing tag (`</style>` or `</script>`). Returns `None` if the tag
/// at `tag_start` is not a style/script opening tag.
fn try_skip_style_or_script(bytes: &[u8], tag_start: usize) -> Option<usize> {
    let remaining = &bytes[tag_start..];
    let remaining_len = remaining.len();

    // Minimum: `<style>` = 7 chars, `<script>` = 8 chars.
    if remaining_len < 7 {
        return None;
    }

    // Must start with '<'.
    if remaining[0] != b'<' {
        return None;
    }

    // Determine which element we matched.
    let (tag_name_len, close_tag): (usize, &[u8]) =
        if remaining_len >= 7 && remaining[1..6].eq_ignore_ascii_case(b"style") {
            // Check the byte after "style": must be whitespace, '>', or '/'.
            let after = remaining[6];
            if after == b'>'
                || after == b' '
                || after == b'\t'
                || after == b'\n'
                || after == b'\r'
                || after == b'/'
            {
                (5, b"</style>")
            } else {
                return None;
            }
        } else if remaining_len >= 8 && remaining[1..7].eq_ignore_ascii_case(b"script") {
            let after = remaining[7];
            if after == b'>'
                || after == b' '
                || after == b'\t'
                || after == b'\n'
                || after == b'\r'
                || after == b'/'
            {
                (6, b"</script>")
            } else {
                return None;
            }
        } else {
            return None;
        };

    // Handle self-closing variant: <style /> or <script />. Find the '>' first.
    if let Some(gt_offset) = memchr::memchr(b'>', remaining) {
        // Check if it's self-closing (ends with "/>").
        if gt_offset > 0 && remaining[gt_offset - 1] == b'/' {
            return Some(tag_start + gt_offset + 1);
        }
    }

    // Find the matching closing tag (case-insensitive).
    let close_tag_len = close_tag.len(); // 8 for </style>, 9 for </script>
    let search_start = tag_start + tag_name_len + 2; // skip past "<style" or "<script"

    // Scan for the closing tag.
    let mut scan = search_start;
    while scan + close_tag_len <= bytes.len() {
        // Look for '<' as the start of a potential closing tag.
        match memchr::memchr(b'<', &bytes[scan..]) {
            Some(offset) => {
                let candidate = scan + offset;
                if candidate + close_tag_len <= bytes.len()
                    && bytes[candidate..candidate + close_tag_len].eq_ignore_ascii_case(close_tag)
                {
                    return Some(candidate + close_tag_len);
                }
                scan = candidate + 1;
            },
            None => break,
        }
    }

    // No closing tag found -- strip from the opening tag to end of input.
    Some(bytes.len())
}

/// Returns `true` if the byte is valid inside an HTML entity name (alphanumeric or `#`).
#[inline(always)]
fn is_entity_char_fast(b: u8) -> bool {
    matches!(b, b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'#')
}

/// Returns `true` if a tag needs normalization (i.e., is a non-self-closed void element).
#[inline]
fn tag_needs_normalization(tag: &str) -> bool {
    let tag_bytes = tag.as_bytes();
    let tag_len = tag_bytes.len();

    // Closing tags and already self-closing tags don't need normalization.
    if tag_len < 3 || tag_bytes[1] == b'/' || tag_bytes[tag_len - 2] == b'/' {
        return false;
    }

    // Extract the element name (after '<', before space or '>').
    let inner = &tag_bytes[1..tag_len - 1];
    let name_end = find_name_end(inner);
    let name_bytes = &inner[..name_end];

    is_void_element_fast(name_bytes)
}

/// Normalizes a tag and appends it directly to the output buffer.
///
/// Called only for tags that `tag_needs_normalization` identified as non-self-closed
/// void elements. Appends `<...attrs />` (replacing trailing `>` with ` />`).
fn normalize_tag_into(output: &mut String, tag: &str) {
    let tag_len = tag.len();
    // Write everything except the closing `>`, then append ` />`.
    output.push_str(&tag[..tag_len - 1]);
    output.push_str(" />");
}

/// Finds the end of the element name within the inner tag bytes (between `<` and `>`).
/// Returns the index of the first whitespace or `/` byte, or the full length.
#[inline(always)]
fn find_name_end(inner: &[u8]) -> usize {
    // For typical short tag names (2-6 chars), a simple scalar loop is faster
    // than SIMD dispatch overhead.
    for (i, &b) in inner.iter().enumerate() {
        if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' || b == b'/' {
            return i;
        }
    }
    inner.len()
}

/// Returns `true` if `name` (case-insensitive) is an HTML void element.
///
/// Uses a two-level dispatch on name length and first byte (lowercased) to
/// avoid the linear scan over all 13 void element names. This replaces the
/// previous implementation that called `eq_ignore_ascii_case` up to 13 times
/// per tag, which accounted for ~1.3% of total CPU.
#[inline]
fn is_void_element_fast(name: &[u8]) -> bool {
    let n = name.len();
    if !(2..=6).contains(&n) {
        return false;
    }

    // Lowercase the first byte for dispatch.
    let first = name[0].to_ascii_lowercase();

    match (n, first) {
        // 2-letter: br, hr
        (2, b'b') => name[1].eq_ignore_ascii_case(&b'r'),
        (2, b'h') => name[1].eq_ignore_ascii_case(&b'r'),
        // 3-letter: img, col, wbr
        (3, b'i') => eq_lower2(&name[1..3], b"mg"),
        (3, b'c') => eq_lower2(&name[1..3], b"ol"),
        (3, b'w') => eq_lower2(&name[1..3], b"br"),
        // 4-letter: meta, link, area, base
        (4, b'm') => eq_lower3(&name[1..4], b"eta"),
        (4, b'l') => eq_lower3(&name[1..4], b"ink"),
        (4, b'a') => eq_lower3(&name[1..4], b"rea"),
        (4, b'b') => eq_lower3(&name[1..4], b"ase"),
        // 5-letter: input, embed, track
        (5, b'i') => eq_lower4(&name[1..5], b"nput"),
        (5, b'e') => eq_lower4(&name[1..5], b"mbed"),
        (5, b't') => eq_lower4(&name[1..5], b"rack"),
        // 6-letter: source
        (6, b's') => name[1..6].eq_ignore_ascii_case(b"ource"),
        _ => false,
    }
}

/// Case-insensitive comparison for exactly 2 bytes.
#[inline(always)]
fn eq_lower2(a: &[u8], lower: &[u8; 2]) -> bool {
    a[0].to_ascii_lowercase() == lower[0] && a[1].to_ascii_lowercase() == lower[1]
}

/// Case-insensitive comparison for exactly 3 bytes.
#[inline(always)]
fn eq_lower3(a: &[u8], lower: &[u8; 3]) -> bool {
    a[0].to_ascii_lowercase() == lower[0]
        && a[1].to_ascii_lowercase() == lower[1]
        && a[2].to_ascii_lowercase() == lower[2]
}

/// Case-insensitive comparison for exactly 4 bytes.
#[inline(always)]
fn eq_lower4(a: &[u8], lower: &[u8; 4]) -> bool {
    a[0].to_ascii_lowercase() == lower[0]
        && a[1].to_ascii_lowercase() == lower[1]
        && a[2].to_ascii_lowercase() == lower[2]
        && a[3].to_ascii_lowercase() == lower[3]
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
    fn normalizer_no_change_returns_borrowed() {
        // Well-formed XHTML should return Cow::Borrowed (zero allocation).
        let input = "<p>Hello &amp; World</p>";
        let result = normalize_xhtml(input);
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn normalizer_plain_text_returns_borrowed() {
        let input = "Just some plain text without any special chars";
        let result = normalize_xhtml(input);
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn normalizer_all_void_elements() {
        // Test all 13 void elements.
        for tag in &[
            "br", "hr", "img", "meta", "link", "input", "area", "base", "col", "embed", "source",
            "track", "wbr",
        ] {
            let input = format!("<{}>", tag);
            let result = normalize_xhtml(&input);
            assert!(
                result.contains(" />"),
                "void element <{}> should be self-closed, got: {}",
                tag,
                result
            );
        }
    }

    #[test]
    fn normalizer_void_elements_case_insensitive() {
        let input = "<BR>";
        let result = normalize_xhtml(input);
        assert_eq!(&*result, "<BR />");

        let input = "<Img src=\"x.png\">";
        let result = normalize_xhtml(input);
        assert_eq!(&*result, "<Img src=\"x.png\" />");
    }

    #[test]
    fn normalizer_non_void_elements_untouched() {
        let input = "<p>text</p>";
        let result = normalize_xhtml(input);
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn normalizer_mixed_bare_amp_and_void_tag() {
        // Regression: bare `&` followed by a void tag must copy intermediate text.
        let input = "A&B<br>C";
        let result = normalize_xhtml(input);
        assert_eq!(&*result, "A&amp;B<br />C");
    }

    #[test]
    fn normalizer_mixed_content() {
        let input = "<p>A & B<br>C &amp; D<hr>E</p>";
        let result = normalize_xhtml(input);
        assert_eq!(&*result, "<p>A &amp; B<br />C &amp; D<hr />E</p>");
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

    // --- style/script stripping tests ---

    #[test]
    fn normalizer_strips_style_block() {
        let input = "<head><style>body { margin: 0; }</style></head><body><p>Hello</p></body>";
        let result = normalize_xhtml(input);
        assert_eq!(&*result, "<head></head><body><p>Hello</p></body>");
    }

    #[test]
    fn normalizer_strips_script_block() {
        let input = "<head><script>alert('xss');</script></head><body><p>Hello</p></body>";
        let result = normalize_xhtml(input);
        assert_eq!(&*result, "<head></head><body><p>Hello</p></body>");
    }

    #[test]
    fn normalizer_preserves_content_around_style_script() {
        let input = "Before<style>css</style>Middle<script>js</script>After";
        let result = normalize_xhtml(input);
        assert_eq!(&*result, "BeforeMiddleAfter");
    }

    #[test]
    fn normalizer_strips_style_case_insensitive() {
        let input = "<STYLE>body{}</STYLE><p>ok</p>";
        let result = normalize_xhtml(input);
        assert_eq!(&*result, "<p>ok</p>");

        let input2 = "<Script>var x=1;</Script><p>ok</p>";
        let result2 = normalize_xhtml(input2);
        assert_eq!(&*result2, "<p>ok</p>");

        let input3 = "<sTyLe>body{}</STYLE><p>ok</p>";
        let result3 = normalize_xhtml(input3);
        assert_eq!(&*result3, "<p>ok</p>");
    }

    #[test]
    fn normalizer_strips_style_with_attributes() {
        let input =
            "<style type=\"text/css\">@page { padding: 0; margin: 0; }</style><p>Content</p>";
        let result = normalize_xhtml(input);
        assert_eq!(&*result, "<p>Content</p>");
    }

    #[test]
    fn normalizer_strips_script_with_attributes() {
        let input = "<script type=\"text/javascript\" src=\"x.js\"></script><p>Content</p>";
        let result = normalize_xhtml(input);
        assert_eq!(&*result, "<p>Content</p>");
    }

    #[test]
    fn normalizer_strips_multiple_style_blocks() {
        let input = "<style>a{}</style><p>A</p><style>b{}</style><p>B</p>";
        let result = normalize_xhtml(input);
        assert_eq!(&*result, "<p>A</p><p>B</p>");
    }

    #[test]
    fn normalizer_strips_style_block_no_closing_tag() {
        // If there's no closing tag, everything from the style tag to the end should be stripped.
        let input = "Before<style>body { color: red; }";
        let result = normalize_xhtml(input);
        assert_eq!(&*result, "Before");
    }

    #[test]
    fn normalizer_does_not_strip_styled_or_stylesheet() {
        // Tags like <styled> or elements with "style" in their name should NOT be stripped.
        let input = "<p styled=\"yes\">text</p>";
        let result = normalize_xhtml(input);
        // Should be unchanged (no style/script stripping).
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn normalizer_strips_style_with_newlines() {
        let input = "<style>\n  body {\n    margin: 0;\n  }\n</style><p>ok</p>";
        let result = normalize_xhtml(input);
        assert_eq!(&*result, "<p>ok</p>");
    }

    #[test]
    fn normalizer_style_strip_combined_with_void_and_amp() {
        // Ensure style stripping works alongside void-element normalization and
        // ampersand escaping in the same document.
        let input = "<style>css</style><p>A & B<br>C</p>";
        let result = normalize_xhtml(input);
        assert_eq!(&*result, "<p>A &amp; B<br />C</p>");
    }
}
