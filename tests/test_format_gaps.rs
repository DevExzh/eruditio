//! Integration tests for AZW4, OEB, and MOBI family writer aliases.

use eruditio::domain::{Book, Chapter, Format};
use eruditio::pipeline::convert::Pipeline;
use eruditio::pipeline::options::ConversionOptions;
use std::io::Cursor;

/// Helper: creates a simple test book.
fn make_test_book() -> Book {
    let mut book = Book::new();
    book.metadata.title = Some("Format Gap Test".into());
    book.metadata.authors.push("Test Author".into());
    book.metadata.language = Some("en".into());
    book.metadata.description = Some("Testing new format implementations.".into());

    book.add_chapter(&Chapter {
        title: Some("Chapter One".into()),
        content: "<p>First chapter of the format gap test.</p>".into(),
        id: Some("ch1".into()),
    });
    book.add_chapter(&Chapter {
        title: Some("Chapter Two".into()),
        content: "<p>Second chapter with more content.</p>".into(),
        id: Some("ch2".into()),
    });

    book
}

// ---------------------------------------------------------------------------
// OEB round-trip
// ---------------------------------------------------------------------------

#[test]
fn oeb_round_trip_through_pipeline() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write OEB.
    let mut oeb_buf = Vec::new();
    pipeline
        .write(Format::Oeb, &book, &mut oeb_buf)
        .expect("write OEB");
    assert!(!oeb_buf.is_empty());

    // Read OEB back.
    let mut input = Cursor::new(oeb_buf);
    let decoded = pipeline
        .read(Format::Oeb, &mut input, &ConversionOptions::default())
        .expect("read OEB");

    assert_eq!(decoded.metadata.title.as_deref(), Some("Format Gap Test"));
    assert_eq!(decoded.metadata.authors, vec!["Test Author"]);
    assert_eq!(decoded.metadata.language.as_deref(), Some("en"));

    let chapters = decoded.chapters();
    assert_eq!(chapters.len(), 2);
    assert!(chapters[0].content.contains("First chapter"));
    assert!(chapters[1].content.contains("Second chapter"));
}

#[test]
fn oeb_to_epub_conversion() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write OEB.
    let mut oeb_buf = Vec::new();
    pipeline
        .write(Format::Oeb, &book, &mut oeb_buf)
        .expect("write OEB");

    // Convert OEB → EPUB.
    let mut input = Cursor::new(oeb_buf);
    let mut output = Vec::new();
    pipeline
        .convert(
            Format::Oeb,
            Format::Epub,
            &mut input,
            &mut output,
            &ConversionOptions::default(),
        )
        .expect("OEB to EPUB conversion");

    // Verify EPUB output.
    assert!(!output.is_empty());
    let mut epub_input = Cursor::new(output);
    let epub_book = pipeline
        .read(Format::Epub, &mut epub_input, &ConversionOptions::default())
        .expect("read EPUB");

    assert_eq!(epub_book.metadata.title.as_deref(), Some("Format Gap Test"));
    let chapters = epub_book.chapters();
    assert!(!chapters.is_empty());
}

#[test]
fn epub_to_oeb_conversion() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write EPUB.
    let mut epub_buf = Vec::new();
    pipeline
        .write(Format::Epub, &book, &mut epub_buf)
        .expect("write EPUB");

    // Convert EPUB → OEB.
    let mut input = Cursor::new(epub_buf);
    let mut output = Vec::new();
    pipeline
        .convert(
            Format::Epub,
            Format::Oeb,
            &mut input,
            &mut output,
            &ConversionOptions::default(),
        )
        .expect("EPUB to OEB conversion");

    // Read back OEB.
    assert!(!output.is_empty());
    let mut oeb_input = Cursor::new(output);
    let oeb_book = pipeline
        .read(Format::Oeb, &mut oeb_input, &ConversionOptions::default())
        .expect("read OEB");

    assert_eq!(oeb_book.metadata.title.as_deref(), Some("Format Gap Test"));
    assert!(!oeb_book.chapters().is_empty());
}

// ---------------------------------------------------------------------------
// MOBI family writer aliases
// ---------------------------------------------------------------------------

#[test]
fn azw_writer_produces_valid_mobi() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write AZW (should use MobiWriter).
    let mut azw_buf = Vec::new();
    pipeline
        .write(Format::Azw, &book, &mut azw_buf)
        .expect("write AZW");
    assert!(!azw_buf.is_empty());

    // Should be readable as MOBI.
    let mut input = Cursor::new(azw_buf);
    let decoded = pipeline
        .read(Format::Mobi, &mut input, &ConversionOptions::default())
        .expect("read as MOBI");
    assert_eq!(decoded.metadata.title.as_deref(), Some("Format Gap Test"));
}

#[test]
fn azw3_writer_produces_valid_mobi() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write AZW3 (should use MobiWriter).
    let mut azw3_buf = Vec::new();
    pipeline
        .write(Format::Azw3, &book, &mut azw3_buf)
        .expect("write AZW3");
    assert!(!azw3_buf.is_empty());

    // Should be readable as AZW3.
    let mut input = Cursor::new(azw3_buf);
    let decoded = pipeline
        .read(Format::Azw3, &mut input, &ConversionOptions::default())
        .expect("read as AZW3");
    assert_eq!(decoded.metadata.title.as_deref(), Some("Format Gap Test"));
}

#[test]
fn prc_writer_produces_valid_mobi() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write PRC (should use MobiWriter).
    let mut prc_buf = Vec::new();
    pipeline
        .write(Format::Prc, &book, &mut prc_buf)
        .expect("write PRC");
    assert!(!prc_buf.is_empty());

    // Should be readable as PRC.
    let mut input = Cursor::new(prc_buf);
    let decoded = pipeline
        .read(Format::Prc, &mut input, &ConversionOptions::default())
        .expect("read as PRC");
    assert_eq!(decoded.metadata.title.as_deref(), Some("Format Gap Test"));
}

// ---------------------------------------------------------------------------
// Registry coverage
// ---------------------------------------------------------------------------

#[test]
fn new_formats_registered_in_registry() {
    let pipeline = Pipeline::new();
    let registry = pipeline.registry();

    // AZW4: read-only
    assert!(registry.can_read(&Format::Azw4), "AZW4 should be readable");
    assert!(
        !registry.can_write(&Format::Azw4),
        "AZW4 should not be writable"
    );

    // OEB: read + write
    assert!(registry.can_read(&Format::Oeb), "OEB should be readable");
    assert!(registry.can_write(&Format::Oeb), "OEB should be writable");

    // AZW: read + write
    assert!(registry.can_read(&Format::Azw), "AZW should be readable");
    assert!(registry.can_write(&Format::Azw), "AZW should be writable");

    // AZW3: read + write
    assert!(registry.can_read(&Format::Azw3), "AZW3 should be readable");
    assert!(registry.can_write(&Format::Azw3), "AZW3 should be writable");

    // PRC: read + write
    assert!(registry.can_read(&Format::Prc), "PRC should be readable");
    assert!(registry.can_write(&Format::Prc), "PRC should be writable");
}
