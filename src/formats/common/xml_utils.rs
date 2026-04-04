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

/// Converts a byte slice to an owned String using the fast path when valid UTF-8.
///
/// Most XML content is valid UTF-8. `str::from_utf8` is cheaper than
/// `from_utf8_lossy` for valid input because it avoids the `Utf8Chunks`
/// iterator overhead.
pub(crate) fn bytes_to_string(bytes: &[u8]) -> String {
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
    let s = std::str::from_utf8(raw).unwrap_or("");
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
