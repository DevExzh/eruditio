// benches/conversion.rs — End-to-end parsing and conversion benchmarks
// with statistical rigor for profiling hot paths and allocation patterns.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use std::io::Cursor;
use std::path::PathBuf;
use std::time::Duration;

use eruditio::domain::FormatReader;
use eruditio::formats::epub::EpubReader;
use eruditio::formats::fb2::Fb2Reader;
use eruditio::formats::html::HtmlReader;
use eruditio::formats::mobi::MobiReader;
use eruditio::formats::rtf::RtfReader;
use eruditio::{ConversionOptions, Format, Pipeline};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")
}

/// Load a file from assets/, returning None if it doesn't exist.
fn load_asset(rel_path: &str) -> Option<Vec<u8>> {
    let path = assets_dir().join(rel_path);
    std::fs::read(&path).ok()
}

/// Load an asset and verify it parses successfully with the given reader.
/// Returns None if the file is missing or fails to parse (e.g. corrupt ZIPs).
fn load_validated<R: FormatReader>(rel_path: &str, reader: &R) -> Option<Vec<u8>> {
    let data = load_asset(rel_path)?;
    let mut cursor = Cursor::new(data.as_slice());
    match reader.read_book(&mut cursor) {
        Ok(_) => Some(data),
        Err(_) => {
            eprintln!("WARN: skipping {rel_path} (parse failed)");
            None
        },
    }
}

/// Generate synthetic RTF of approximately `size` bytes.
fn make_rtf(size: usize) -> Vec<u8> {
    let mut rtf = Vec::with_capacity(size);
    rtf.extend_from_slice(b"{\\rtf1\\ansi\\deff0 ");
    let text = b"This is paragraph text with some formatting. ";
    let ctrl = b"\\par\\b Bold content here\\b0 \\i italic\\i0 ";
    let mut i = 0u32;
    while rtf.len() < size.saturating_sub(10) {
        if i % 5 == 0 {
            rtf.extend_from_slice(ctrl);
        } else {
            rtf.extend_from_slice(text);
        }
        i += 1;
    }
    rtf.push(b'}');
    rtf
}

// ---------------------------------------------------------------------------
// Format parsing: EPUB
// ---------------------------------------------------------------------------

fn bench_epub_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("epub_parsing");
    group.sample_size(200);
    group.measurement_time(Duration::from_secs(10));

    let reader = EpubReader::new();
    let cases: &[(&str, &str)] = &[
        ("small_13k", "epub/epub30-test-0204.epub"),
        ("medium_135k", "epub/pg-alice-in-wonderland.epub"),
        ("large_1m", "epub/epub30-test-0100.epub"),
    ];

    for &(name, path) in cases {
        if let Some(data) = load_validated(path, &reader) {
            // Reduce sample size for large files to keep total bench time reasonable.
            if data.len() > 1_000_000 {
                group.sample_size(50);
                group.measurement_time(Duration::from_secs(15));
            }

            group.bench_with_input(BenchmarkId::new("reader", name), &data, |b, d| {
                b.iter(|| {
                    let mut cursor = Cursor::new(black_box(d.as_slice()));
                    EpubReader::new().read_book(&mut cursor).unwrap()
                })
            });
        }
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Format parsing: HTML
// ---------------------------------------------------------------------------

fn bench_html_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("html_parsing");
    group.sample_size(200);
    group.measurement_time(Duration::from_secs(10));

    let reader = HtmlReader::new();
    let cases: &[(&str, &str)] = &[
        ("medium_148k", "html/metamorphosis.html"),
        ("large_941k", "html/emma.html"),
    ];

    for &(name, path) in cases {
        if let Some(data) = load_validated(path, &reader) {
            group.bench_with_input(BenchmarkId::new("reader", name), &data, |b, d| {
                b.iter(|| {
                    let mut cursor = Cursor::new(black_box(d.as_slice()));
                    HtmlReader::new().read_book(&mut cursor).unwrap()
                })
            });
        }
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Format parsing: FB2
// ---------------------------------------------------------------------------

fn bench_fb2_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("fb2_parsing");
    group.sample_size(200);
    group.measurement_time(Duration::from_secs(10));

    let reader = Fb2Reader::new();
    let cases: &[(&str, &str)] = &[
        ("small_86k", "fb2/sample_book_fileexamples.fb2"),
        ("large_1m", "fb2/alice_wonderland.fb2"),
    ];

    for &(name, path) in cases {
        if let Some(data) = load_validated(path, &reader) {
            group.bench_with_input(BenchmarkId::new("reader", name), &data, |b, d| {
                b.iter(|| {
                    let mut cursor = Cursor::new(black_box(d.as_slice()));
                    Fb2Reader::new().read_book(&mut cursor).unwrap()
                })
            });
        }
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Format parsing: MOBI
// ---------------------------------------------------------------------------

fn bench_mobi_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("mobi_parsing");
    group.sample_size(100);
    group.measurement_time(Duration::from_secs(10));

    let reader = MobiReader::new();
    let cases: &[(&str, &str)] = &[
        ("medium_1m", "mobi/sample_marketing_strategies.mobi"),
        (
            "kindle_huffdic",
            "kindle_test_files/azw/sample-unicode-huffdic.mobi",
        ),
    ];

    for &(name, path) in cases {
        if let Some(data) = load_validated(path, &reader) {
            group.bench_with_input(BenchmarkId::new("reader", name), &data, |b, d| {
                b.iter(|| {
                    let mut cursor = Cursor::new(black_box(d.as_slice()));
                    MobiReader::new().read_book(&mut cursor).unwrap()
                })
            });
        }
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Format parsing: RTF (synthetic)
// ---------------------------------------------------------------------------

fn bench_rtf_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("rtf_parsing");
    group.sample_size(200);
    group.measurement_time(Duration::from_secs(10));

    let rtf_10k = make_rtf(10_000);
    let rtf_100k = make_rtf(100_000);

    group.bench_with_input(BenchmarkId::new("reader", "10k"), &rtf_10k, |b, d| {
        b.iter(|| {
            let mut cursor = Cursor::new(black_box(d.as_slice()));
            RtfReader::new().read_book(&mut cursor).unwrap()
        })
    });

    group.bench_with_input(BenchmarkId::new("reader", "100k"), &rtf_100k, |b, d| {
        b.iter(|| {
            let mut cursor = Cursor::new(black_box(d.as_slice()));
            RtfReader::new().read_book(&mut cursor).unwrap()
        })
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Pipeline conversion: end-to-end format-to-format
// ---------------------------------------------------------------------------

fn bench_pipeline_conversion(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline_conversion");
    group.sample_size(100);
    group.measurement_time(Duration::from_secs(10));

    let pipeline = Pipeline::new();
    let opts_none = ConversionOptions::none();
    let opts_all = ConversionOptions::all();

    // Helper: validate a full pipeline conversion before benchmarking it.
    let try_convert = |data: &[u8], from: Format, to: Format, opts: &ConversionOptions| -> bool {
        let mut input = Cursor::new(data);
        let mut output = Vec::new();
        pipeline
            .convert(from, to, &mut input, &mut output, opts)
            .is_ok()
    };

    // HTML -> EPUB (common conversion path)
    if let Some(html_data) = load_asset("html/metamorphosis.html") {
        if try_convert(&html_data, Format::Html, Format::Epub, &opts_none) {
            group.bench_function("html_to_epub/148k_no_transforms", |b| {
                b.iter(|| {
                    let mut input = Cursor::new(black_box(html_data.as_slice()));
                    let mut output = Vec::with_capacity(html_data.len());
                    pipeline
                        .convert(
                            Format::Html,
                            Format::Epub,
                            &mut input,
                            &mut output,
                            &opts_none,
                        )
                        .unwrap()
                })
            });
        }

        if try_convert(&html_data, Format::Html, Format::Epub, &opts_all) {
            group.bench_function("html_to_epub/148k_all_transforms", |b| {
                b.iter(|| {
                    let mut input = Cursor::new(black_box(html_data.as_slice()));
                    let mut output = Vec::with_capacity(html_data.len());
                    pipeline
                        .convert(
                            Format::Html,
                            Format::Epub,
                            &mut input,
                            &mut output,
                            &opts_all,
                        )
                        .unwrap()
                })
            });
        }
    }

    // FB2 -> EPUB
    if let Some(fb2_data) = load_asset("fb2/alice_wonderland.fb2") {
        if try_convert(&fb2_data, Format::Fb2, Format::Epub, &opts_all) {
            group.bench_function("fb2_to_epub/1m", |b| {
                b.iter(|| {
                    let mut input = Cursor::new(black_box(fb2_data.as_slice()));
                    let mut output = Vec::with_capacity(fb2_data.len());
                    pipeline
                        .convert(
                            Format::Fb2,
                            Format::Epub,
                            &mut input,
                            &mut output,
                            &opts_all,
                        )
                        .unwrap()
                })
            });
        }
    }

    // EPUB -> FB2
    if let Some(epub_data) = load_asset("epub/pg-alice-in-wonderland.epub") {
        if try_convert(&epub_data, Format::Epub, Format::Fb2, &opts_all) {
            group.bench_function("epub_to_fb2/135k", |b| {
                b.iter(|| {
                    let mut input = Cursor::new(black_box(epub_data.as_slice()));
                    let mut output = Vec::with_capacity(epub_data.len());
                    pipeline
                        .convert(
                            Format::Epub,
                            Format::Fb2,
                            &mut input,
                            &mut output,
                            &opts_all,
                        )
                        .unwrap()
                })
            });
        }
    }

    // RTF -> EPUB (synthetic)
    let rtf_50k = make_rtf(50_000);
    if try_convert(&rtf_50k, Format::Rtf, Format::Epub, &opts_all) {
        group.bench_function("rtf_to_epub/50k_synthetic", |b| {
            b.iter(|| {
                let mut input = Cursor::new(black_box(rtf_50k.as_slice()));
                let mut output = Vec::with_capacity(rtf_50k.len());
                pipeline
                    .convert(
                        Format::Rtf,
                        Format::Epub,
                        &mut input,
                        &mut output,
                        &opts_all,
                    )
                    .unwrap()
            })
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Pipeline: read-only (parse + transform, no write)
// ---------------------------------------------------------------------------

fn bench_pipeline_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline_read");
    group.sample_size(200);
    group.measurement_time(Duration::from_secs(10));

    let pipeline = Pipeline::new();
    let opts_all = ConversionOptions::all();

    // Helper: validate a pipeline read before benchmarking.
    let try_read = |data: &[u8], fmt: Format, opts: &ConversionOptions| -> bool {
        let mut cursor = Cursor::new(data);
        pipeline.read(fmt, &mut cursor, opts).is_ok()
    };

    if let Some(epub_data) = load_asset("epub/pg-alice-in-wonderland.epub") {
        if try_read(&epub_data, Format::Epub, &opts_all) {
            group.bench_function("epub/135k_all_transforms", |b| {
                b.iter(|| {
                    let mut cursor = Cursor::new(black_box(epub_data.as_slice()));
                    pipeline.read(Format::Epub, &mut cursor, &opts_all).unwrap()
                })
            });
        }
    }

    if let Some(html_data) = load_asset("html/emma.html") {
        if try_read(&html_data, Format::Html, &opts_all) {
            group.bench_function("html/941k_all_transforms", |b| {
                b.iter(|| {
                    let mut cursor = Cursor::new(black_box(html_data.as_slice()));
                    pipeline.read(Format::Html, &mut cursor, &opts_all).unwrap()
                })
            });
        }
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Criterion harness
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_epub_parsing,
    bench_html_parsing,
    bench_fb2_parsing,
    bench_mobi_parsing,
    bench_rtf_parsing,
    bench_pipeline_conversion,
    bench_pipeline_read,
);
criterion_main!(benches);
