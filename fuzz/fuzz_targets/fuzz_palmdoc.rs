#![no_main]
use libfuzzer_sys::fuzz_target;

use eruditio::formats::common::compression::palmdoc;

fuzz_target!(|data: &[u8]| {
    // 1. Decompress arbitrary data — must not panic.
    let _ = palmdoc::decompress(data);

    // 2. Round-trip: compress then decompress must reproduce the original.
    //    PalmDoc records are at most 4096 bytes.
    if data.len() <= 4096 {
        let compressed = palmdoc::compress(data);
        let decompressed = palmdoc::decompress(&compressed)
            .expect("decompressing our own output must succeed");
        assert_eq!(
            &decompressed, data,
            "round-trip mismatch: compress then decompress diverged"
        );
    }
});
