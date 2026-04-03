// benches/intrinsics.rs

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use eruditio::formats::common::text_utils;

// ---------------------------------------------------------------------------
// case-insensitive search (exercises case_fold + byte_scan indirectly)
// ---------------------------------------------------------------------------

fn bench_find_case_insensitive(c: &mut Criterion) {
    // Short needle in medium haystack.
    let haystack_1k: Vec<u8> = b"The quick brown fox jumps over the lazy dog. "
        .repeat(23);
    let needle_short = b"LAZY DOG";

    c.bench_function("find_ci/short_needle_1k", |b| {
        b.iter(|| text_utils::find_case_insensitive(
            black_box(&haystack_1k), black_box(needle_short)
        ))
    });

    // Longer haystack (10k) with needle near the end.
    let haystack_10k: Vec<u8> = b"abcdefghij klmnopqrst uvwxyz0123 456789ABCD "
        .repeat(222);
    let needle_mid = b"789ABCD";

    c.bench_function("find_ci/mid_needle_10k", |b| {
        b.iter(|| text_utils::find_case_insensitive(
            black_box(&haystack_10k), black_box(needle_mid)
        ))
    });

    // Needle not found (worst case — full scan).
    let needle_missing = b"ZZZZZ";
    c.bench_function("find_ci/missing_needle_10k", |b| {
        b.iter(|| text_utils::find_case_insensitive(
            black_box(&haystack_10k), black_box(needle_missing)
        ))
    });
}

// ---------------------------------------------------------------------------
// XML escape with 5-byte set (exercises byte_scan directly)
// ---------------------------------------------------------------------------

fn bench_escape_xml(c: &mut Criterion) {
    // XML mode uses 5-byte set (&<>"') — the primary byte_scan use case.
    let mixed = "Hello &amp; welcome to the <b>world</b> of \"ebooks\". ".repeat(200);
    let clean = "This is plain text without any special characters at all ok. ".repeat(167);

    c.bench_function("escape_xml/10k_mixed", |b| {
        b.iter(|| text_utils::escape_xml(black_box(&mixed)))
    });
    c.bench_function("escape_xml/10k_clean", |b| {
        b.iter(|| text_utils::escape_xml(black_box(&clean)))
    });
}

criterion_group!(benches, bench_find_case_insensitive, bench_escape_xml);
criterion_main!(benches);
