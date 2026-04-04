//! Memory profiling tests using DHAT.
//!
//! Run with: `cargo test --features dhat-heap --test memory -- --test-threads=1`
//!
//! IMPORTANT: dhat requires single-threaded execution (`--test-threads=1`) because
//! there can only be one active `dhat::Profiler` at a time.

#![cfg(feature = "dhat-heap")]

use eruditio::EruditioParser;
use std::io::Cursor;

/// Helper: generate a synthetic plain-text "book" of approximately `size_bytes`.
fn synthetic_txt(size_bytes: usize) -> Vec<u8> {
    let line = "All happy families are alike; each unhappy family is unhappy in its own way.\n";
    let mut buf = Vec::with_capacity(size_bytes);
    while buf.len() < size_bytes {
        let remaining = size_bytes - buf.len();
        if remaining >= line.len() {
            buf.extend_from_slice(line.as_bytes());
        } else {
            buf.extend_from_slice(&line.as_bytes()[..remaining]);
        }
    }
    buf
}

/// Helper: build a minimal valid EPUB as an in-memory ZIP.
fn synthetic_epub(chapter_count: usize, chapter_size: usize) -> Vec<u8> {
    use std::io::Write;
    let buf = Vec::new();
    let cursor = Cursor::new(buf);
    let mut zip = zip::ZipWriter::new(cursor);

    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // mimetype (must be first, uncompressed, no extra field)
    zip.start_file("mimetype", options).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();

    // container.xml
    zip.start_file("META-INF/container.xml", options).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
    )
    .unwrap();

    // Generate chapter content
    let para =
        "<p>".to_string() + &"Lorem ipsum dolor sit amet. ".repeat(chapter_size / 30) + "</p>\n";

    // Build manifest and spine entries
    let mut manifest_entries = String::new();
    let mut spine_entries = String::new();
    for i in 0..chapter_count {
        manifest_entries.push_str(&format!(
            r#"    <item id="ch{i}" href="ch{i}.xhtml" media-type="application/xhtml+xml"/>"#
        ));
        manifest_entries.push('\n');
        spine_entries.push_str(&format!(r#"    <itemref idref="ch{i}"/>"#));
        spine_entries.push('\n');
    }

    // content.opf
    zip.start_file("OEBPS/content.opf", options).unwrap();
    let opf = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="uid">urn:uuid:test-memory</dc:identifier>
    <dc:title>Memory Test Book</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
{manifest_entries}  </manifest>
  <spine>
{spine_entries}  </spine>
</package>"#
    );
    zip.write_all(opf.as_bytes()).unwrap();

    // Chapter XHTML files
    for i in 0..chapter_count {
        zip.start_file(format!("OEBPS/ch{i}.xhtml"), options)
            .unwrap();
        let xhtml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Chapter {i}</title></head>
<body>
<h1>Chapter {i}</h1>
{para}
</body>
</html>"#
        );
        zip.write_all(xhtml.as_bytes()).unwrap();
    }

    let cursor = zip.finish().unwrap();
    cursor.into_inner()
}

// ---------------------------------------------------------------------------
// Buffered mode tests
// ---------------------------------------------------------------------------

#[test]
fn buffered_txt_peak_heap_proportional() {
    let _profiler = dhat::Profiler::builder().testing().build();

    let input_size = 1_000_000; // 1 MB
    let data = synthetic_txt(input_size);
    let mut cursor = Cursor::new(&data);

    let book = EruditioParser::parse(&mut cursor, Some("txt")).expect("TXT parse failed");
    let chapter_bytes: usize = book.chapters().iter().map(|c| c.content.len()).sum();

    // Buffered: peak heap should be < 5x input (generous for String re-encoding overhead)
    let stats = dhat::HeapStats::get();
    eprintln!(
        "[buffered_txt] input={}  peak_heap={}  chapter_bytes={}  total_allocs={}",
        input_size, stats.max_bytes, chapter_bytes, stats.total_blocks
    );
    dhat::assert!(
        stats.max_bytes < input_size * 5,
        "Peak heap {} exceeds 5x input size {}",
        stats.max_bytes,
        input_size
    );
}

#[test]
fn buffered_epub_peak_heap_bounded() {
    let _profiler = dhat::Profiler::builder().testing().build();

    // ~2 MB EPUB (20 chapters × 100 KB each)
    let epub_data = synthetic_epub(20, 100_000);
    let input_size = epub_data.len();
    let mut cursor = Cursor::new(&epub_data);

    let book = EruditioParser::parse(&mut cursor, Some("epub")).expect("EPUB parse failed");
    let chapter_count = book.chapter_count();

    let stats = dhat::HeapStats::get();
    eprintln!(
        "[buffered_epub] input={}  peak_heap={}  chapters={}  total_allocs={}",
        input_size, stats.max_bytes, chapter_count, stats.total_blocks
    );
    // Buffered: peak heap should be < 5x input (ZIP decompression + DOM copies)
    dhat::assert!(
        stats.max_bytes < input_size * 5,
        "Peak heap {} exceeds 5x input size {}",
        stats.max_bytes,
        input_size
    );
}

#[test]
fn buffered_allocation_count_reasonable() {
    let _profiler = dhat::Profiler::builder().testing().build();

    let epub_data = synthetic_epub(10, 50_000);
    let input_mb = epub_data.len() as f64 / 1_000_000.0;
    let mut cursor = Cursor::new(&epub_data);

    let _book = EruditioParser::parse(&mut cursor, Some("epub")).expect("EPUB parse failed");

    let stats = dhat::HeapStats::get();
    let allocs_per_mb = stats.total_blocks as f64 / input_mb;
    eprintln!(
        "[alloc_count] input={:.2}MB  total_allocs={}  allocs_per_mb={:.0}",
        input_mb, stats.total_blocks, allocs_per_mb
    );
    // Sanity check: should not be outrageously allocation-heavy
    // Allow up to 50,000 allocations per MB (generous baseline for first measurement)
    dhat::assert!(
        allocs_per_mb < 50_000.0,
        "Allocation rate {:.0}/MB is excessive",
        allocs_per_mb
    );
}

// ---------------------------------------------------------------------------
// Streaming mode placeholders (for future implementation)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "streaming API not yet implemented"]
fn streaming_memory_constant() {
    let _profiler = dhat::Profiler::builder().testing().build();

    // TODO: Once streaming API exists:
    // 1. Process a large file chunk-by-chunk
    // 2. Verify peak heap stays < 10MB regardless of input size
    // 3. Compare 10MB vs 100MB inputs — peak should be similar

    let _stats = dhat::HeapStats::get();
    // dhat::assert!(stats.max_bytes < 10_000_000);
}

#[test]
#[ignore = "streaming API not yet implemented"]
fn streaming_vs_buffered_scaling() {
    let _profiler = dhat::Profiler::builder().testing().build();

    // TODO: Parse same file in both modes, verify:
    // - Streaming peak << Buffered peak for large files
    // - Both produce identical output

    let _stats = dhat::HeapStats::get();
}
