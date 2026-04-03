// benches/intrinsics.rs — Direct benchmarks for all 6 intrinsic operations.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use eruditio::formats::common::text_utils;

// ---------------------------------------------------------------------------
// case_fold: eq_ignore_ascii_case via find_case_insensitive
// ---------------------------------------------------------------------------

fn bench_case_fold(c: &mut Criterion) {
    // 64-byte equal slices with random case.
    let a_64: Vec<u8> = b"The Quick Brown Fox Jumps Over The Lazy Dog! Hello World Boo!!OK"
        .to_vec();
    let b_64: Vec<u8> = b"tHE qUICK bROWN fOX jUMPS oVER tHE lAZY dOG! hELLO wORLD bOO!!ok"
        .to_vec();

    c.bench_function("case_fold/eq_64", |bench| {
        bench.iter(|| {
            // Verify equality through find_case_insensitive (needle = full haystack).
            text_utils::find_case_insensitive(black_box(&a_64), black_box(&b_64))
        })
    });

    // 1024-byte slices.
    let a_1k: Vec<u8> = a_64.repeat(16);
    let b_1k: Vec<u8> = b_64.repeat(16);

    c.bench_function("case_fold/eq_1k", |bench| {
        bench.iter(|| {
            text_utils::find_case_insensitive(black_box(&a_1k), black_box(&b_1k))
        })
    });
}

// ---------------------------------------------------------------------------
// byte_scan: find_first_in_set via escape_xml
// ---------------------------------------------------------------------------

fn bench_byte_scan(c: &mut Criterion) {
    // 10K realistic HTML with scattered special chars.
    let html_10k = "Hello &amp; welcome to the <b>world</b> of \"ebooks\". ".repeat(200);

    c.bench_function("byte_scan/find_5_in_10k", |bench| {
        bench.iter(|| text_utils::escape_xml(black_box(&html_10k)))
    });

    // 10K clean text (no hits — full scan).
    let clean_10k = "This is plain text without any special characters at all ok. ".repeat(167);

    c.bench_function("byte_scan/clean_10k", |bench| {
        bench.iter(|| text_utils::escape_xml(black_box(&clean_10k)))
    });
}

// ---------------------------------------------------------------------------
// cp1252: decode_cp1252
// ---------------------------------------------------------------------------

fn bench_cp1252(c: &mut Criterion) {
    // 10K pure ASCII.
    let ascii_10k: Vec<u8> = b"The quick brown fox jumps over the lazy dog. "
        .repeat(223)[..10_000]
        .to_vec();

    c.bench_function("cp1252/decode_10k_ascii", |bench| {
        bench.iter(|| text_utils::decode_cp1252(black_box(&ascii_10k)))
    });

    // 10K mixed: ~20% non-ASCII bytes.
    let mut mixed_10k = ascii_10k.clone();
    for (i, b) in mixed_10k.iter_mut().enumerate() {
        if i % 5 == 0 {
            *b = 0x93; // left double quote in cp1252
        }
    }

    c.bench_function("cp1252/decode_10k_mixed", |bench| {
        bench.iter(|| text_utils::decode_cp1252(black_box(&mixed_10k)))
    });
}

// ---------------------------------------------------------------------------
// hex_decode: decode_hex_pairs
// ---------------------------------------------------------------------------

fn bench_hex_decode(c: &mut Criterion) {
    // 10K dense hex (no whitespace).
    let hex_10k: String = (0..5000)
        .map(|i| format!("{:02x}", (i % 256) as u8))
        .collect();

    c.bench_function("hex_decode/dense_10k", |bench| {
        bench.iter(|| text_utils::decode_hex_pairs(black_box(&hex_10k)))
    });
}

// ---------------------------------------------------------------------------
// find_case_insensitive (exercises case_fold + memchr)
// ---------------------------------------------------------------------------

fn bench_find_case_insensitive(c: &mut Criterion) {
    let haystack_1k: Vec<u8> = b"The quick brown fox jumps over the lazy dog. "
        .repeat(23);
    let needle_short = b"LAZY DOG";

    c.bench_function("find_ci/short_needle_1k", |bench| {
        bench.iter(|| {
            text_utils::find_case_insensitive(black_box(&haystack_1k), black_box(needle_short))
        })
    });

    let haystack_10k: Vec<u8> = b"abcdefghij klmnopqrst uvwxyz0123 456789ABCD "
        .repeat(222);
    let needle_mid = b"789ABCD";

    c.bench_function("find_ci/mid_needle_10k", |bench| {
        bench.iter(|| {
            text_utils::find_case_insensitive(black_box(&haystack_10k), black_box(needle_mid))
        })
    });

    let needle_missing = b"ZZZZZ";
    c.bench_function("find_ci/missing_needle_10k", |bench| {
        bench.iter(|| {
            text_utils::find_case_insensitive(black_box(&haystack_10k), black_box(needle_missing))
        })
    });
}

// ---------------------------------------------------------------------------
// is_ascii: is_all_ascii
// ---------------------------------------------------------------------------

fn bench_is_ascii(c: &mut Criterion) {
    // 1024 pure ASCII bytes.
    let ascii_1k: Vec<u8> = b"The quick brown fox jumps over the lazy dog. "
        .repeat(23)[..1024]
        .to_vec();

    c.bench_function("is_ascii/scalar_1k", |bench| {
        bench.iter(|| {
            let all_ascii = black_box(&ascii_1k).iter().all(|&b| b < 0x80);
            black_box(all_ascii);
        })
    });

    c.bench_function("is_ascii/simd_1k", |bench| {
        bench.iter(|| {
            black_box(text_utils::is_all_ascii(black_box(&ascii_1k)));
        })
    });

    // 1023 ASCII + 1 non-ASCII at end.
    let mut fail_last = ascii_1k.clone();
    fail_last[1023] = 0x80;

    c.bench_function("is_ascii/scalar_1k_fail_last", |bench| {
        bench.iter(|| {
            let all_ascii = black_box(&fail_last).iter().all(|&b| b < 0x80);
            black_box(all_ascii);
        })
    });

    c.bench_function("is_ascii/simd_1k_fail_last", |bench| {
        bench.iter(|| {
            black_box(text_utils::is_all_ascii(black_box(&fail_last)));
        })
    });

    // 64 bytes pure ASCII.
    let ascii_64: Vec<u8> = b"The quick brown fox jumps over the lazy dog. Hello World!!!OK!!!"
        [..64]
        .to_vec();

    c.bench_function("is_ascii/scalar_64", |bench| {
        bench.iter(|| {
            let all_ascii = black_box(&ascii_64).iter().all(|&b| b < 0x80);
            black_box(all_ascii);
        })
    });

    c.bench_function("is_ascii/simd_64", |bench| {
        bench.iter(|| {
            black_box(text_utils::is_all_ascii(black_box(&ascii_64)));
        })
    });
}

// ---------------------------------------------------------------------------
// skip_ws: skip_whitespace
// ---------------------------------------------------------------------------

fn bench_skip_ws(c: &mut Criterion) {
    // 64 whitespace bytes + 1 non-WS.
    let mut ws_64: Vec<u8> = vec![b' '; 64];
    ws_64.push(b'x');

    c.bench_function("skip_ws/scalar_64", |bench| {
        bench.iter(|| {
            let count = black_box(&ws_64)
                .iter()
                .take_while(|&&b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
                .count();
            black_box(count);
        })
    });

    c.bench_function("skip_ws/simd_64", |bench| {
        bench.iter(|| {
            black_box(text_utils::skip_whitespace(black_box(&ws_64)));
        })
    });

    // 1024 whitespace bytes.
    let ws_1k: Vec<u8> = vec![b' '; 1024];

    c.bench_function("skip_ws/scalar_1k", |bench| {
        bench.iter(|| {
            let count = black_box(&ws_1k)
                .iter()
                .take_while(|&&b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
                .count();
            black_box(count);
        })
    });

    c.bench_function("skip_ws/simd_1k", |bench| {
        bench.iter(|| {
            black_box(text_utils::skip_whitespace(black_box(&ws_1k)));
        })
    });

    // 1024 non-WS bytes (early exit).
    let no_ws: Vec<u8> = vec![b'a'; 1024];

    c.bench_function("skip_ws/scalar_none", |bench| {
        bench.iter(|| {
            let count = black_box(&no_ws)
                .iter()
                .take_while(|&&b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
                .count();
            black_box(count);
        })
    });

    c.bench_function("skip_ws/simd_none", |bench| {
        bench.iter(|| {
            black_box(text_utils::skip_whitespace(black_box(&no_ws)));
        })
    });
}

// ---------------------------------------------------------------------------
// short_pattern: find_short_pattern
// ---------------------------------------------------------------------------

fn bench_short_pattern(c: &mut Criterion) {
    // 10K HTML-like data with scattered closing tags.
    let html_10k = "<p>Hello world</p><div>Content here</div><span>More text</span>".repeat(160);
    let html_bytes = html_10k.as_bytes();

    c.bench_function("short_pat/scalar_2b_10k", |bench| {
        bench.iter(|| {
            let result = black_box(html_bytes)
                .windows(2)
                .position(|w| w == b"</");
            black_box(result);
        })
    });

    c.bench_function("short_pat/simd_2b_10k", |bench| {
        bench.iter(|| {
            black_box(text_utils::find_short_pattern(black_box(html_bytes), b"</"));
        })
    });

    // 10K data for 4-byte pattern.
    let xml_10k = "<item>data</item><!-- comment --><item>more</item>".repeat(200);
    let xml_bytes = xml_10k.as_bytes();

    c.bench_function("short_pat/scalar_4b_10k", |bench| {
        bench.iter(|| {
            let result = black_box(xml_bytes)
                .windows(4)
                .position(|w| w == b"<!--");
            black_box(result);
        })
    });

    c.bench_function("short_pat/simd_4b_10k", |bench| {
        bench.iter(|| {
            black_box(text_utils::find_short_pattern(black_box(xml_bytes), b"<!--"));
        })
    });

    // 10K with no match (full scan).
    let no_match_10k: Vec<u8> = b"abcdefghij klmnopqrst uvwxyz0123 456789ABCD "
        .repeat(222);

    c.bench_function("short_pat/scalar_2b_miss_10k", |bench| {
        bench.iter(|| {
            let result = black_box(&no_match_10k[..])
                .windows(2)
                .position(|w| w == b"</");
            black_box(result);
        })
    });

    c.bench_function("short_pat/simd_2b_miss_10k", |bench| {
        bench.iter(|| {
            black_box(text_utils::find_short_pattern(black_box(&no_match_10k), b"</"));
        })
    });
}

criterion_group!(
    benches,
    bench_case_fold,
    bench_byte_scan,
    bench_cp1252,
    bench_hex_decode,
    bench_find_case_insensitive,
    bench_is_ascii,
    bench_skip_ws,
    bench_short_pattern,
);
criterion_main!(benches);
