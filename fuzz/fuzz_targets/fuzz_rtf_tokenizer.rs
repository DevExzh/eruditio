#![no_main]
use libfuzzer_sys::fuzz_target;

use eruditio::formats::rtf::tokenizer;

fuzz_target!(|data: &[u8]| {
    // tokenize must not panic on arbitrary bytes.
    let tokens = tokenizer::tokenize(data);
    // Smoke check: token count should be non-negative (always true,
    // but forces the optimizer not to discard the result).
    let _ = tokens.len();
});
