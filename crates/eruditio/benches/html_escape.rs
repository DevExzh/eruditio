use criterion::{Criterion, black_box, criterion_group, criterion_main};
use eruditio::formats::common::text_utils;

fn make_html_mixed(size: usize) -> String {
    let chunk = "<p>Hello &amp; welcome to the <b>world</b> of ebooks.</p>\n";
    let mut s = String::with_capacity(size);
    while s.len() < size {
        s.push_str(chunk);
    }
    s.truncate(size);
    s
}

fn make_html_clean(size: usize) -> String {
    let chunk = "This is plain text without any special characters at all. ";
    let mut s = String::with_capacity(size);
    while s.len() < size {
        s.push_str(chunk);
    }
    s.truncate(size);
    s
}

fn make_html_dense(size: usize) -> String {
    let chunk = "&<>&<>&<>&<>&<>&<>&<>&<>";
    let mut s = String::with_capacity(size);
    while s.len() < size {
        s.push_str(chunk);
    }
    s.truncate(size);
    s
}

fn bench_escape_html(c: &mut Criterion) {
    let mixed = make_html_mixed(10_000);
    let clean = make_html_clean(10_000);
    let dense = make_html_dense(10_000);

    c.bench_function("escape_html/10k_mixed", |b| {
        b.iter(|| text_utils::escape_html(black_box(&mixed)))
    });
    c.bench_function("escape_html/10k_clean", |b| {
        b.iter(|| text_utils::escape_html(black_box(&clean)))
    });
    c.bench_function("escape_html/10k_dense", |b| {
        b.iter(|| text_utils::escape_html(black_box(&dense)))
    });
}

fn bench_strip_tags(c: &mut Criterion) {
    let html = make_html_mixed(10_000);
    c.bench_function("strip_tags/10k_html", |b| {
        b.iter(|| text_utils::strip_tags(black_box(&html)))
    });

    let large = make_html_mixed(100_000);
    c.bench_function("strip_tags/100k_html", |b| {
        b.iter(|| text_utils::strip_tags(black_box(&large)))
    });
}

fn bench_unescape(c: &mut Criterion) {
    let text = "Hello &amp; welcome &lt;world&gt; of &quot;ebooks&quot;. ".repeat(200);
    c.bench_function("unescape_entities/10k", |b| {
        b.iter(|| text_utils::unescape_basic_entities(black_box(&text)))
    });
}

fn bench_decode_cp1252(c: &mut Criterion) {
    let mut data = vec![0u8; 10_000];
    for (i, b) in data.iter_mut().enumerate() {
        *b = (i % 256) as u8;
    }
    c.bench_function("decode_cp1252/10k", |b| {
        b.iter(|| text_utils::decode_cp1252(black_box(&data)))
    });
}

fn bench_hex_decode(c: &mut Criterion) {
    let hex: String = (0..5000u16).map(|i| format!("{:02x}", i % 256)).collect();
    c.bench_function("decode_hex_pairs/10k_chars", |b| {
        b.iter(|| text_utils::decode_hex_pairs(black_box(&hex)))
    });
}

criterion_group!(
    benches,
    bench_escape_html,
    bench_strip_tags,
    bench_unescape,
    bench_decode_cp1252,
    bench_hex_decode
);
criterion_main!(benches);
