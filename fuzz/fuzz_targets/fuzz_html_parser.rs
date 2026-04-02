#![no_main]
use libfuzzer_sys::fuzz_target;

use eruditio::formats::html::parser;
use eruditio::Metadata;

fuzz_target!(|data: &[u8]| {
    if let Ok(html) = std::str::from_utf8(data) {
        // extract_metadata must not panic on arbitrary HTML.
        let _meta = parser::extract_metadata(html);

        // extract_body must not panic.
        let body = parser::extract_body(html);

        // split_into_chapters must not panic.
        let chapters = parser::split_into_chapters(&body);
        // Each chapter must have non-empty content.
        for (_, content) in &chapters {
            assert!(!content.trim().is_empty());
        }

        // build_html_document must not panic.
        let meta = Metadata::default();
        let doc = parser::build_html_document("fuzz", &meta, &body);
        assert!(doc.contains("<!DOCTYPE html>"));
    }
});
