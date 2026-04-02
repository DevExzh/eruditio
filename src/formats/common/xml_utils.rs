use quick_xml::events::BytesStart;

/// Extracts the value of an attribute from a quick-xml `BytesStart` element.
/// Returns `None` if the attribute is not present.
pub fn get_attribute(element: &BytesStart<'_>, name: &str) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attr| {
            let key = String::from_utf8_lossy(attr.key.as_ref());
            key == name
        })
        .map(|attr| String::from_utf8_lossy(&attr.value).into_owned())
}

/// Extracts all attributes from a quick-xml element into a Vec of (key, value) pairs.
pub fn get_all_attributes(element: &BytesStart<'_>) -> Vec<(String, String)> {
    element
        .attributes()
        .flatten()
        .map(|attr| {
            let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
            let value = String::from_utf8_lossy(&attr.value).into_owned();
            (key, value)
        })
        .collect()
}

/// Extracts the local name from a potentially namespaced tag (e.g. "dc:title" -> "title").
pub fn local_name(tag: &str) -> &str {
    tag.rsplit_once(':').map_or(tag, |(_, local)| local)
}

/// Escapes special XML characters in text content.
pub fn escape_xml(text: &str) -> String {
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
