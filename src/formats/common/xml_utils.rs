#![allow(dead_code)]
use quick_xml::events::BytesStart;

/// Extracts the value of an attribute from a quick-xml `BytesStart` element.
/// Returns `None` if the attribute is not present.
///
/// Compares attribute keys at the byte level (XML names are always ASCII),
/// avoiding a `from_utf8_lossy` conversion per key.
pub(crate) fn get_attribute(element: &BytesStart<'_>, name: &str) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attr| attr.key.as_ref() == name.as_bytes())
        .map(|attr| bytes_to_string(&attr.value))
}

/// Extracts all attributes from a quick-xml element into a Vec of (key, value) pairs.
pub(crate) fn get_all_attributes(element: &BytesStart<'_>) -> Vec<(String, String)> {
    element
        .attributes()
        .flatten()
        .map(|attr| {
            let key = bytes_to_string(attr.key.as_ref());
            let value = bytes_to_string(&attr.value);
            (key, value)
        })
        .collect()
}

/// Converts a byte slice to an owned String using the fastest available path.
///
/// Three-tier strategy:
/// 1. **ASCII fast path** (SIMD-accelerated): if every byte is < 0x80 we can
///    skip UTF-8 validation entirely and wrap the bytes directly.
/// 2. **UTF-8 fast path**: `str::from_utf8` validates without allocating; if
///    valid we just `.to_string()` once.
/// 3. **Lossy fallback**: only reached for genuinely malformed input, using
///    `Utf8Chunks` internally.
pub(crate) fn bytes_to_string(bytes: &[u8]) -> String {
    // Tier 1: SIMD ASCII check -- skips UTF-8 validation entirely.
    if super::intrinsics::is_ascii::is_all_ascii(bytes) {
        // SAFETY: all bytes are < 0x80, which is valid UTF-8.
        return unsafe { std::str::from_utf8_unchecked(bytes) }.to_owned();
    }
    // Tier 2: full UTF-8 validation (still cheaper than lossy for valid input).
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Extracts the local name from a potentially namespaced tag (e.g. "dc:title" -> "title").
pub(crate) fn local_name(tag: &str) -> &str {
    tag.rsplit_once(':').map_or(tag, |(_, local)| local)
}

/// Extracts the local tag name from a raw XML byte slice, zero-allocation.
///
/// XML tag names are always valid UTF-8 in well-formed documents. Falls back
/// to an empty string for invalid UTF-8 (should never happen in practice).
pub(crate) fn local_tag_name(raw: &[u8]) -> &str {
    // XML tag names are always ASCII in well-formed documents, so skip
    // full UTF-8 validation when every byte is < 0x80.
    let s = if super::intrinsics::is_ascii::is_all_ascii(raw) {
        // SAFETY: all bytes are < 0x80, which is valid UTF-8.
        unsafe { std::str::from_utf8_unchecked(raw) }
    } else {
        std::str::from_utf8(raw).unwrap_or("")
    };
    local_name(s)
}

/// Escapes special XML characters in text content.
pub(crate) fn escape_xml(text: &str) -> std::borrow::Cow<'_, str> {
    super::text_utils::escape_xml(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_name_strips_namespace() {
        assert_eq!(local_name("dc:title"), "title");
        assert_eq!(local_name("title"), "title");
        assert_eq!(local_name("opf:meta"), "meta");
    }

    #[test]
    fn escape_xml_handles_all_chars() {
        assert_eq!(
            escape_xml("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&apos;f"
        );
    }

    #[test]
    fn escape_xml_no_op_for_clean_text() {
        assert_eq!(escape_xml("hello world"), "hello world");
    }
}
