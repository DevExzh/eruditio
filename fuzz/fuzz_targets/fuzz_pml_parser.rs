#![no_main]
use libfuzzer_sys::fuzz_target;

use eruditio::formats::pml::parser;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        // pml_to_html must not panic on arbitrary PML markup.
        let html = parser::pml_to_html(text);
        let _ = html.len();
    }
});
