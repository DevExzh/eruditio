use eruditio::domain::FormatReader;
use eruditio::domain::FormatWriter;
use eruditio::domain::{Book, Chapter, Format, TocItem};
use eruditio::formats::{EpubReader, EpubWriter};
use eruditio::pipeline::convert::Pipeline;
use eruditio::pipeline::options::ConversionOptions;
use std::io::Cursor;
use std::path::Path;

#[test]
fn epub_round_trip_preserves_metadata() {
    let mut book = Book::new();
    book.metadata.title = Some("EPUB Round Trip".into());
    book.metadata.authors.push("Jane Doe".into());
    book.metadata.language = Some("en".into());
    book.metadata.description = Some("A test book for EPUB round-tripping.".into());

    book.add_chapter(Chapter {
        title: Some("Chapter 1".into()),
        content: "<p>First chapter content.</p>".into(),
        id: Some("ch1".into()),
    });
    book.add_chapter(Chapter {
        title: Some("Chapter 2".into()),
        content: "<p>Second chapter content.</p>".into(),
        id: Some("ch2".into()),
    });

    book.add_resource(
        "cover_img",
        "images/cover.png",
        vec![0x89, 0x50, 0x4E, 0x47],
        "image/png",
    );

    // Write to EPUB
    let mut epub_bytes = Vec::new();
    EpubWriter::new()
        .write_book(&book, &mut epub_bytes)
        .expect("Failed to write EPUB");

    // Read back
    let mut cursor = Cursor::new(epub_bytes);
    let decoded = EpubReader::new()
        .read_book(&mut cursor)
        .expect("Failed to read EPUB");

    // Verify metadata
    assert_eq!(decoded.metadata.title.as_deref(), Some("EPUB Round Trip"));
    assert!(decoded.metadata.authors.iter().any(|a| a.contains("Jane")));
    assert_eq!(decoded.metadata.language.as_deref(), Some("en"));

    // Verify chapters
    let chapters = decoded.chapters();
    assert_eq!(chapters.len(), 2);
    assert!(chapters[0].content.contains("First chapter content"));
    assert!(chapters[1].content.contains("Second chapter content"));

    // Verify TOC
    assert!(!decoded.toc.is_empty());

    // Verify resource was preserved
    let cover_data = decoded.resource_data("cover_img");
    assert!(cover_data.is_some());
    assert_eq!(cover_data.unwrap(), &[0x89, 0x50, 0x4E, 0x47]);
}

#[test]
fn epub_round_trip_single_chapter() {
    let mut book = Book::new();
    book.metadata.title = Some("Minimal".into());
    book.add_chapter(Chapter {
        title: Some("Only Chapter".into()),
        content: "<p>All content here.</p>".into(),
        id: Some("only".into()),
    });

    let mut epub_bytes = Vec::new();
    EpubWriter::new()
        .write_book(&book, &mut epub_bytes)
        .expect("write");

    let mut cursor = Cursor::new(epub_bytes);
    let decoded = EpubReader::new().read_book(&mut cursor).expect("read");

    assert_eq!(decoded.metadata.title.as_deref(), Some("Minimal"));
    assert_eq!(decoded.chapters().len(), 1);
    assert!(decoded.chapters()[0].content.contains("All content here"));
}

/// Helper: recursively counts all TOC items in a tree.
fn count_toc_items(items: &[TocItem]) -> usize {
    items
        .iter()
        .map(|item| 1 + count_toc_items(&item.children))
        .sum()
}

/// Regression test: EPUB round-trip through the pipeline (with TocGenerator)
/// should not duplicate NCX navpoints. Before the fix, TOC entries with
/// fragment hrefs (e.g. "ch1.html#anchor") were not recognized as covering
/// the same document as bare spine hrefs (e.g. "ch1.html"), causing the
/// TocGenerator to add duplicate entries.
#[test]
fn epub_round_trip_no_toc_duplication() {
    let path = Path::new("test-data/real-world/small/pg-alice-in-wonderland.epub");
    if !path.exists() {
        eprintln!("WARNING: pg-alice-in-wonderland.epub not found; skipping test");
        return;
    }

    let pipeline = Pipeline::new();

    // Read the source EPUB.
    let source_bytes = std::fs::read(path).expect("read epub file");
    let mut cursor = Cursor::new(source_bytes.clone());
    let source_book = pipeline
        .read(Format::Epub, &mut cursor, &ConversionOptions::none())
        .expect("parse source epub");
    let source_toc_count = count_toc_items(&source_book.toc);

    // Round-trip: EPUB -> EPUB with all transforms (including TocGenerator).
    let mut input = Cursor::new(source_bytes);
    let mut output = Vec::new();
    let _result = pipeline
        .convert(
            Format::Epub,
            Format::Epub,
            &mut input,
            &mut output,
            &ConversionOptions::all(),
        )
        .expect("convert EPUB -> EPUB");

    // Read the round-tripped EPUB.
    let mut verify = Cursor::new(output);
    let rt_book = pipeline
        .read(Format::Epub, &mut verify, &ConversionOptions::none())
        .expect("read round-tripped epub");
    let rt_toc_count = count_toc_items(&rt_book.toc);

    eprintln!(
        "Alice TOC: source={}, round-trip={}",
        source_toc_count, rt_toc_count
    );

    // The round-trip TOC should not have significantly more entries than
    // the source. Allow a small tolerance (e.g., +2) for legitimately new
    // entries like a cover wrapper, but definitely not double.
    assert!(
        rt_toc_count <= source_toc_count + 2,
        "TOC duplication detected: source had {} entries but round-trip has {} \
         (expected at most {} + 2 = {})",
        source_toc_count,
        rt_toc_count,
        source_toc_count,
        source_toc_count + 2
    );
}

/// Same test for Frankenstein.
#[test]
fn epub_round_trip_no_toc_duplication_frankenstein() {
    let path = Path::new("test-data/real-world/small/pg-frankenstein.epub");
    if !path.exists() {
        eprintln!("WARNING: pg-frankenstein.epub not found; skipping test");
        return;
    }

    let pipeline = Pipeline::new();

    let source_bytes = std::fs::read(path).expect("read epub file");
    let mut cursor = Cursor::new(source_bytes.clone());
    let source_book = pipeline
        .read(Format::Epub, &mut cursor, &ConversionOptions::none())
        .expect("parse source epub");
    let source_toc_count = count_toc_items(&source_book.toc);

    let mut input = Cursor::new(source_bytes);
    let mut output = Vec::new();
    let _result = pipeline
        .convert(
            Format::Epub,
            Format::Epub,
            &mut input,
            &mut output,
            &ConversionOptions::all(),
        )
        .expect("convert EPUB -> EPUB");

    let mut verify = Cursor::new(output);
    let rt_book = pipeline
        .read(Format::Epub, &mut verify, &ConversionOptions::none())
        .expect("read round-tripped epub");
    let rt_toc_count = count_toc_items(&rt_book.toc);

    eprintln!(
        "Frankenstein TOC: source={}, round-trip={}",
        source_toc_count, rt_toc_count
    );

    assert!(
        rt_toc_count <= source_toc_count + 2,
        "TOC duplication detected: source had {} entries but round-trip has {} \
         (expected at most {} + 2 = {})",
        source_toc_count,
        rt_toc_count,
        source_toc_count,
        source_toc_count + 2
    );
}
