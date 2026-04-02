#![no_main]
use libfuzzer_sys::fuzz_target;

use eruditio::formats::common::text_utils;

fuzz_target!(|data: &[u8]| {
    // Test with arbitrary bytes interpreted as UTF-8 where valid.
    if let Ok(text) = std::str::from_utf8(data) {
        // escape_html must not panic; output is at least as long as input.
        let escaped = text_utils::escape_html(text);
        assert!(escaped.len() >= text.len());

        // escape_xml must not panic; output is at least as long as input.
        let xml_escaped = text_utils::escape_xml(text);
        assert!(xml_escaped.len() >= text.len());

        // strip_tags must not panic; output is at most as long as input.
        let stripped = text_utils::strip_tags(text);
        assert!(stripped.len() <= text.len());

        // unescape_basic_entities must not panic.
        let _ = text_utils::unescape_basic_entities(text);

        // find_case_insensitive must not panic.
        // Use raw byte slices to avoid slicing mid-character.
        if data.len() >= 3 {
            let _ = text_utils::find_case_insensitive(data, &data[..3]);
        }

        // Round-trip: escape then unescape should recover the original
        // for text that has no pre-existing entities.
        if !text.contains('&') {
            let round_tripped = text_utils::unescape_basic_entities(&escaped);
            assert_eq!(round_tripped, text, "escape_html/unescape round-trip mismatch");
        }
    }

    // decode_cp1252 works on arbitrary bytes — must not panic.
    let _ = text_utils::decode_cp1252(data);

    // decode_hex_pairs on arbitrary bytes interpreted as hex string.
    if let Ok(hex_str) = std::str::from_utf8(data) {
        let _ = text_utils::decode_hex_pairs(hex_str);
    }

    // cp1252_byte_to_char on every byte — must not panic.
    for &b in data.iter().take(256) {
        let _ = text_utils::cp1252_byte_to_char(b);
    }
});
