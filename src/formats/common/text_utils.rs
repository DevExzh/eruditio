//! Shared text processing utilities optimized with `memchr` for SIMD-accelerated
//! byte scanning. Consolidates duplicate escape, strip, decode functions from
//! format-specific modules.

use memchr::{memchr, memchr2};

// ---------------------------------------------------------------------------
// HTML / XML escaping (single-pass, one allocation)
// ---------------------------------------------------------------------------

/// Escapes `&`, `<`, `>` for safe embedding in HTML body content.
///
/// Single-pass implementation using `memchr` to skip over runs of safe bytes
/// in bulk, replacing the chained `.replace()` pattern (which allocates 3
/// intermediate `String`s) with a single allocation.
pub fn escape_html(text: &str) -> String {
    escape_impl(text, false)
}

/// Escapes `&`, `<`, `>`, `"`, `'` for safe embedding in XML/HTML attributes.
pub fn escape_xml(text: &str) -> String {
    escape_impl(text, true)
}

fn escape_impl(text: &str, xml_mode: bool) -> String {
    let bytes = text.as_bytes();
    let len = bytes.len();

    let set: &[u8] = if xml_mode {
        b"&<>\"'"
    } else {
        b"&<>"
    };

    // Fast path: no special characters found.
    if !super::intrinsics::byte_scan::has_any_in_set(bytes, set) {
        return text.to_string();
    }

    let mut result = String::with_capacity(len + len / 8);
    let mut pos = 0;

    while pos < len {
        let next = super::intrinsics::byte_scan::find_first_in_set(&bytes[pos..], set);

        match next {
            Some(offset) => {
                // Copy safe prefix in bulk.
                result.push_str(&text[pos..pos + offset]);
                let ch = bytes[pos + offset];
                match ch {
                    b'&' => result.push_str("&amp;"),
                    b'<' => result.push_str("&lt;"),
                    b'>' => result.push_str("&gt;"),
                    b'"' => result.push_str("&quot;"),
                    b'\'' => result.push_str("&apos;"),
                    _ => {}
                }
                pos += offset + 1;
            }
            None => {
                result.push_str(&text[pos..]);
                break;
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// HTML tag stripping (memchr-accelerated)
// ---------------------------------------------------------------------------

/// Strips all HTML/XML tags, returning plain text.
///
/// Uses `memchr` to jump between `<` and `>` delimiters instead of scanning
/// character by character.
pub fn strip_tags(html: &str) -> String {
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut result = String::with_capacity(len);
    let mut pos = 0;

    loop {
        // Find the next '<' (tag start).
        match memchr(b'<', &bytes[pos..]) {
            Some(offset) => {
                // Copy text before the tag.
                if offset > 0 {
                    result.push_str(&html[pos..pos + offset]);
                }
                let tag_start = pos + offset;
                // Find the closing '>'.
                match memchr(b'>', &bytes[tag_start..]) {
                    Some(end_offset) => {
                        pos = tag_start + end_offset + 1;
                    },
                    None => {
                        // Unclosed tag — skip the rest.
                        break;
                    },
                }
            },
            None => {
                // No more tags — copy remaining text.
                result.push_str(&html[pos..]);
                break;
            },
        }
    }

    result
}

// ---------------------------------------------------------------------------
// HTML entity unescaping
// ---------------------------------------------------------------------------

/// Decodes the most common HTML/XML character entities.
pub fn unescape_basic_entities(text: &str) -> String {
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Fast path: no ampersands means nothing to unescape.
    if memchr(b'&', bytes).is_none() {
        return text.to_string();
    }

    let mut result = String::with_capacity(len);
    let mut pos = 0;

    while pos < len {
        match memchr(b'&', &bytes[pos..]) {
            Some(offset) => {
                result.push_str(&text[pos..pos + offset]);
                let entity_start = pos + offset;
                // Find the semicolon.
                let rest = &text[entity_start..];
                if let Some(semi) = rest.find(';') {
                    let entity = &rest[..semi + 1];
                    match entity {
                        "&amp;" => result.push('&'),
                        "&lt;" => result.push('<'),
                        "&gt;" => result.push('>'),
                        "&quot;" => result.push('"'),
                        "&apos;" => result.push('\''),
                        "&nbsp;" => result.push('\u{00A0}'),
                        _ => result.push_str(entity), // unknown — keep as-is
                    }
                    pos = entity_start + semi + 1;
                } else {
                    // No semicolon found — copy the '&' literally.
                    result.push('&');
                    pos = entity_start + 1;
                }
            },
            None => {
                result.push_str(&text[pos..]);
                break;
            },
        }
    }

    result
}

// ---------------------------------------------------------------------------
// CP-1252 decoding (delegated to intrinsics for SIMD acceleration)
// ---------------------------------------------------------------------------

/// Converts a single CP-1252 byte to its Unicode character.
#[inline]
pub fn cp1252_byte_to_char(byte: u8) -> char {
    super::intrinsics::cp1252::cp1252_byte_to_char(byte)
}

/// Decodes a CP-1252 (Windows-1252) byte slice to a Unicode `String`.
pub fn decode_cp1252(data: &[u8]) -> String {
    super::intrinsics::cp1252::decode_cp1252(data)
}

// ---------------------------------------------------------------------------
// Hex decoding (delegated to intrinsics)
// ---------------------------------------------------------------------------

/// Decodes pairs of hex ASCII characters into bytes.
///
/// Skips whitespace between hex digits. Non-hex characters (other than
/// whitespace) cause the pair to be skipped.
pub fn decode_hex_pairs(hex: &str) -> Vec<u8> {
    super::intrinsics::hex_decode::decode_hex_pairs(hex)
}

// ---------------------------------------------------------------------------
// ASCII detection (delegated to intrinsics for SIMD acceleration)
// ---------------------------------------------------------------------------

/// Returns `true` if every byte in `data` is in the ASCII range (0x00–0x7F).
pub fn is_all_ascii(data: &[u8]) -> bool {
    super::intrinsics::is_ascii::is_all_ascii(data)
}

// ---------------------------------------------------------------------------
// Whitespace skipping (delegated to intrinsics for SIMD acceleration)
// ---------------------------------------------------------------------------

/// Returns the number of leading XML-whitespace bytes (0x20, 0x09, 0x0A, 0x0D).
pub fn skip_whitespace(data: &[u8]) -> usize {
    super::intrinsics::skip_ws::skip_whitespace(data)
}

// ---------------------------------------------------------------------------
// Short pattern search (delegated to intrinsics for SIMD acceleration)
// ---------------------------------------------------------------------------

/// Finds the first occurrence of a 2–4 byte `needle` in `haystack`.
pub fn find_short_pattern(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    super::intrinsics::short_pattern::find_short_pattern(haystack, needle)
}

// ---------------------------------------------------------------------------
// Case-insensitive ASCII search (allocation-free)
// ---------------------------------------------------------------------------

/// Finds `needle` in `haystack` using ASCII case-insensitive comparison.
///
/// Both `haystack` and `needle` must be valid UTF-8 byte slices. This function
/// only folds ASCII letters (A-Z / a-z); non-ASCII bytes are compared exactly.
///
/// Uses `memchr` to quickly locate candidate start positions based on the first
/// byte of the needle.
pub fn find_case_insensitive(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }

    let first_lower = needle[0].to_ascii_lowercase();
    let first_upper = needle[0].to_ascii_uppercase();

    let mut pos = 0;
    loop {
        // Use memchr2 to find the next candidate position matching first byte
        // in either case.
        let offset = if first_lower == first_upper {
            memchr(first_lower, &haystack[pos..])?
        } else {
            memchr2(first_lower, first_upper, &haystack[pos..])?
        };
        let candidate = pos + offset;

        if candidate + needle.len() > haystack.len() {
            return None;
        }

        // Compare the rest of the needle case-insensitively.
        let matched = super::intrinsics::case_fold::eq_ignore_ascii_case(
            &haystack[candidate..candidate + needle.len()],
            needle,
        );

        if matched {
            return Some(candidate);
        }

        pos = candidate + 1;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- escape_html ---------------------------------------------------------

    #[test]
    fn escape_html_empty() {
        assert_eq!(escape_html(""), "");
    }

    #[test]
    fn escape_html_no_special() {
        assert_eq!(escape_html("hello world"), "hello world");
    }

    #[test]
    fn escape_html_all_special() {
        assert_eq!(escape_html("&<>"), "&amp;&lt;&gt;");
    }

    #[test]
    fn escape_html_mixed() {
        assert_eq!(escape_html("a & b < c > d"), "a &amp; b &lt; c &gt; d");
    }

    #[test]
    fn escape_html_unicode() {
        assert_eq!(
            escape_html("caf\u{00E9} & th\u{00E9}"),
            "caf\u{00E9} &amp; th\u{00E9}"
        );
    }

    // -- escape_xml ----------------------------------------------------------

    #[test]
    fn escape_xml_quotes() {
        assert_eq!(escape_xml("a\"b'c"), "a&quot;b&apos;c");
    }

    // -- strip_tags ----------------------------------------------------------

    #[test]
    fn strip_tags_basic() {
        assert_eq!(strip_tags("<p>Hello <b>world</b></p>"), "Hello world");
    }

    #[test]
    fn strip_tags_no_tags() {
        assert_eq!(strip_tags("no tags here"), "no tags here");
    }

    #[test]
    fn strip_tags_nested() {
        assert_eq!(strip_tags("<div><p>A</p><p>B</p></div>"), "AB");
    }

    #[test]
    fn strip_tags_empty() {
        assert_eq!(strip_tags(""), "");
    }

    #[test]
    fn strip_tags_unclosed() {
        assert_eq!(strip_tags("before <unclosed"), "before ");
    }

    // -- unescape_basic_entities ---------------------------------------------

    #[test]
    fn unescape_common() {
        assert_eq!(
            unescape_basic_entities("&amp; &lt; &gt; &quot;"),
            "& < > \""
        );
    }

    #[test]
    fn unescape_no_entities() {
        assert_eq!(unescape_basic_entities("hello"), "hello");
    }

    #[test]
    fn unescape_unknown_entity() {
        assert_eq!(unescape_basic_entities("&foo;"), "&foo;");
    }

    // -- decode_cp1252 -------------------------------------------------------

    #[test]
    fn cp1252_ascii() {
        assert_eq!(decode_cp1252(b"Hello"), "Hello");
    }

    #[test]
    fn cp1252_special_bytes() {
        // 0x93 = left double quote, 0x94 = right double quote
        assert_eq!(decode_cp1252(&[0x93, 0x94]), "\u{201C}\u{201D}");
    }

    #[test]
    fn cp1252_euro() {
        assert_eq!(cp1252_byte_to_char(0x80), '\u{20AC}');
    }

    // -- decode_hex_pairs ----------------------------------------------------

    #[test]
    fn hex_decode_basic() {
        assert_eq!(decode_hex_pairs("48656c6c6f"), b"Hello");
    }

    #[test]
    fn hex_decode_with_whitespace() {
        assert_eq!(decode_hex_pairs("48 65 6c 6c 6f"), b"Hello");
    }

    #[test]
    fn hex_decode_uppercase() {
        assert_eq!(decode_hex_pairs("4F4B"), b"OK");
    }

    #[test]
    fn hex_decode_empty() {
        assert_eq!(decode_hex_pairs(""), b"");
    }

    // -- find_case_insensitive -----------------------------------------------

    #[test]
    fn case_insensitive_basic() {
        assert_eq!(find_case_insensitive(b"Hello World", b"hello"), Some(0));
    }

    #[test]
    fn case_insensitive_middle() {
        assert_eq!(find_case_insensitive(b"foo BAR baz", b"bar"), Some(4));
    }

    #[test]
    fn case_insensitive_no_match() {
        assert_eq!(find_case_insensitive(b"Hello", b"xyz"), None);
    }

    #[test]
    fn case_insensitive_empty_needle() {
        assert_eq!(find_case_insensitive(b"Hello", b""), None);
    }
}
