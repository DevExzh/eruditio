// benches/intrinsics_integration.rs — End-to-end benchmarks measuring
// the impact of SIMD intrinsic wiring on format-level operations.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use eruditio::formats::common::text_utils;

// ---------------------------------------------------------------------------
// find_case_insensitive: realistic tag search in large HTML
// ---------------------------------------------------------------------------

fn bench_find_ci_in_html(c: &mut Criterion) {
    // 50KB HTML, searching for a tag near the end.
    let mut html = String::with_capacity(51_000);
    for _ in 0..500 {
        html.push_str("<div class=\"item\"><p>Some content here.</p></div>\n");
    }
    html.push_str("<SCRIPT type=\"text/javascript\">alert('end');</SCRIPT>\n");
    let bytes = html.as_bytes();

    c.bench_function("find_ci/script_in_50k_html", |bench| {
        bench.iter(|| text_utils::find_case_insensitive(black_box(bytes), black_box(b"<script")))
    });

    // Search for something that doesn't exist in the document.
    c.bench_function("find_ci/missing_in_50k_html", |bench| {
        bench.iter(|| text_utils::find_case_insensitive(black_box(bytes), black_box(b"<FRAMESET")))
    });
}

// ---------------------------------------------------------------------------
// HTML round-trip through FormatReader (exercises find_case_insensitive
// in extract_body, extract_meta_tags, extract_tag_content)
// ---------------------------------------------------------------------------

fn bench_html_reader_50k(c: &mut Criterion) {
    use eruditio::domain::FormatReader;
    use eruditio::formats::html::HtmlReader;
    use std::io::Cursor;

    let body_content =
        "<p>Lorem ipsum dolor sit amet, consectetur adipiscing elit. </p>\n".repeat(500);
    let html_50k = format!(
        "<HTML><HEAD><TITLE>Test Book</TITLE>\
         <META NAME=\"author\" CONTENT=\"Test Author\">\
         <META NAME=\"description\" CONTENT=\"A test book for benchmarking\">\
         </HEAD><BODY>{}</BODY></HTML>",
        body_content
    );
    let html_bytes = html_50k.into_bytes();

    c.bench_function("html/reader_50k", |bench| {
        bench.iter(|| {
            let mut cursor = Cursor::new(black_box(&html_bytes));
            HtmlReader::new().read_book(&mut cursor).unwrap()
        })
    });
}

// ---------------------------------------------------------------------------
// escape_xml on realistic document content
// ---------------------------------------------------------------------------

fn bench_escape_xml_realistic(c: &mut Criterion) {
    // Realistic ebook chapter with some entities
    let chapter =
        "<p>He said, \"Hello &amp; welcome!\" — it's a <em>wonderful</em> day.</p>\n".repeat(200);

    c.bench_function("escape_xml/chapter_14k", |bench| {
        bench.iter(|| text_utils::escape_xml(black_box(&chapter)))
    });
}

criterion_group!(
    benches,
    bench_find_ci_in_html,
    bench_html_reader_50k,
    bench_escape_xml_realistic,
);
criterion_main!(benches);
