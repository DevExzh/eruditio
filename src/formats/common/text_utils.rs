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

    // Fast path: scan for any special character. If none, return as-is.
    // Match only the characters that will actually be escaped in this mode.
    let has_special = if xml_mode {
        bytes
            .iter()
            .any(|&b| matches!(b, b'&' | b'<' | b'>' | b'"' | b'\''))
    } else {
        bytes.iter().any(|&b| matches!(b, b'&' | b'<' | b'>'))
    };
    if !has_special {
        return text.to_string();
    }

    let mut result = String::with_capacity(len + len / 8);
    let mut pos = 0;

    while pos < len {
        // Find the next byte that needs escaping.
        let next = bytes[pos..].iter().position(|&b| {
            b == b'&' || b == b'<' || b == b'>' || (xml_mode && (b == b'"' || b == b'\''))
        });

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
                    _ => {}, // position() guarantees only matched bytes reach here
                }
                pos += offset + 1;
            },
            None => {
                // No more special chars — copy the rest and done.
                result.push_str(&text[pos..]);
                break;
            },
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
// CP-1252 decoding (static lookup table)
// ---------------------------------------------------------------------------

/// Windows-1252 to Unicode lookup table. Bytes 0x00-0x7F and 0xA0-0xFF map
/// directly to the same Unicode code point. Bytes 0x80-0x9F have special
/// mappings.
static CP1252_TABLE: [char; 256] = {
    let mut table = ['\0'; 256];
    let mut i = 0u16;
    while i < 256 {
        table[i as usize] = i as u8 as char;
        i += 1;
    }
    // Special mappings for 0x80-0x9F range.
    table[0x80] = '\u{20AC}'; // Euro sign
    // 0x81 is undefined in CP-1252; map to U+FFFD replacement character.
    table[0x81] = '\u{FFFD}';
    table[0x82] = '\u{201A}'; // Single low-9 quotation mark
    table[0x83] = '\u{0192}'; // Latin small letter f with hook
    table[0x84] = '\u{201E}'; // Double low-9 quotation mark
    table[0x85] = '\u{2026}'; // Horizontal ellipsis
    table[0x86] = '\u{2020}'; // Dagger
    table[0x87] = '\u{2021}'; // Double dagger
    table[0x88] = '\u{02C6}'; // Modifier letter circumflex accent
    table[0x89] = '\u{2030}'; // Per mille sign
    table[0x8A] = '\u{0160}'; // Latin capital letter S with caron
    table[0x8B] = '\u{2039}'; // Single left-pointing angle quotation
    table[0x8C] = '\u{0152}'; // Latin capital ligature OE
    // 0x8D is undefined in CP-1252; map to U+FFFD replacement character.
    table[0x8D] = '\u{FFFD}';
    table[0x8E] = '\u{017D}'; // Latin capital letter Z with caron
    // 0x8F is undefined in CP-1252; map to U+FFFD replacement character.
    table[0x8F] = '\u{FFFD}';
    // 0x90 is undefined in CP-1252; map to U+FFFD replacement character.
    table[0x90] = '\u{FFFD}';
    table[0x91] = '\u{2018}'; // Left single quotation mark
    table[0x92] = '\u{2019}'; // Right single quotation mark
    table[0x93] = '\u{201C}'; // Left double quotation mark
    table[0x94] = '\u{201D}'; // Right double quotation mark
    table[0x95] = '\u{2022}'; // Bullet
    table[0x96] = '\u{2013}'; // En dash
    table[0x97] = '\u{2014}'; // Em dash
    table[0x98] = '\u{02DC}'; // Small tilde
    table[0x99] = '\u{2122}'; // Trade mark sign
    table[0x9A] = '\u{0161}'; // Latin small letter s with caron
    table[0x9B] = '\u{203A}'; // Single right-pointing angle quotation
    table[0x9C] = '\u{0153}'; // Latin small ligature oe
    table[0x9E] = '\u{017E}'; // Latin small letter z with caron
    table[0x9F] = '\u{0178}'; // Latin capital letter Y with diaeresis
    table
};

/// Converts a single CP-1252 byte to its Unicode character.
#[inline]
pub fn cp1252_byte_to_char(byte: u8) -> char {
    CP1252_TABLE[byte as usize]
}

/// Decodes a CP-1252 (Windows-1252) byte slice to a Unicode `String`.
///
/// Uses a static 256-entry lookup table for O(1) per byte with no branch
/// misprediction overhead.
pub fn decode_cp1252(data: &[u8]) -> String {
    let mut result = String::with_capacity(data.len());
    for &b in data {
        result.push(CP1252_TABLE[b as usize]);
    }
    result
}

// ---------------------------------------------------------------------------
// Hex decoding (lookup-table based)
// ---------------------------------------------------------------------------

/// Lookup table: ASCII byte value -> hex nibble value (0-15), or 0xFF for
/// non-hex bytes.
static HEX_VAL: [u8; 256] = {
    let mut table = [0xFF_u8; 256];
    table[b'0' as usize] = 0;
    table[b'1' as usize] = 1;
    table[b'2' as usize] = 2;
    table[b'3' as usize] = 3;
    table[b'4' as usize] = 4;
    table[b'5' as usize] = 5;
    table[b'6' as usize] = 6;
    table[b'7' as usize] = 7;
    table[b'8' as usize] = 8;
    table[b'9' as usize] = 9;
    table[b'a' as usize] = 10;
    table[b'b' as usize] = 11;
    table[b'c' as usize] = 12;
    table[b'd' as usize] = 13;
    table[b'e' as usize] = 14;
    table[b'f' as usize] = 15;
    table[b'A' as usize] = 10;
    table[b'B' as usize] = 11;
    table[b'C' as usize] = 12;
    table[b'D' as usize] = 13;
    table[b'E' as usize] = 14;
    table[b'F' as usize] = 15;
    table
};

/// Decodes pairs of hex ASCII characters into bytes.
///
/// Skips whitespace between hex digits. Non-hex characters (other than
/// whitespace) cause the pair to be skipped.
pub fn decode_hex_pairs(hex: &str) -> Vec<u8> {
    let bytes = hex.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        // Skip whitespace.
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if i + 1 >= bytes.len() {
            break;
        }
        let hi = HEX_VAL[bytes[i] as usize];
        let lo = HEX_VAL[bytes[i + 1] as usize];
        if hi != 0xFF && lo != 0xFF {
            out.push((hi << 4) | lo);
        }
        i += 2;
    }
    out
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
        let matched = haystack[candidate..candidate + needle.len()]
            .iter()
            .zip(needle)
            .all(|(a, b)| a.eq_ignore_ascii_case(b));

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
