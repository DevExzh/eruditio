use eruditio::domain::FormatReader;
use eruditio::domain::FormatWriter;
use eruditio::domain::{Book, Chapter};
use eruditio::formats::{EpubReader, EpubWriter};
use std::io::Cursor;

#[test]
fn epub_round_trip_preserves_metadata() {
    let mut book = Book::new();
    book.metadata.title = Some("EPUB Round Trip".into());
    book.metadata.authors.push("Jane Doe".into());
    book.metadata.language = Some("en".into());
    book.metadata.description = Some("A test book for EPUB round-tripping.".into());

    book.add_chapter(&Chapter {
        title: Some("Chapter 1".into()),
        content: "<p>First chapter content.</p>".into(),
        id: Some("ch1".into()),
    });
    book.add_chapter(&Chapter {
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
    book.add_chapter(&Chapter {
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
