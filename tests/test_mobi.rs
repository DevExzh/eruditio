use eruditio::domain::{Book, Chapter};
use eruditio::domain::{FormatReader, FormatWriter};
use eruditio::formats::{MobiReader, MobiWriter};
use std::io::Cursor;

#[test]
fn mobi_round_trip_preserves_metadata() {
    let mut book = Book::new();
    book.metadata.title = Some("MOBI Integration Test".into());
    book.metadata.authors.push("Test Author".into());
    book.metadata.language = Some("en".into());

    book.add_chapter(&Chapter {
        title: Some("Chapter 1".into()),
        content: "<p>First chapter of the MOBI integration test.</p>".into(),
        id: Some("ch1".into()),
    });
    book.add_chapter(&Chapter {
        title: Some("Chapter 2".into()),
        content: "<p>Second chapter with different content.</p>".into(),
        id: Some("ch2".into()),
    });

    // Write
    let mut mobi_bytes = Vec::new();
    MobiWriter::new()
        .write_book(&book, &mut mobi_bytes)
        .expect("Failed to write MOBI");

    // Read back
    let mut cursor = Cursor::new(mobi_bytes);
    let decoded = MobiReader::new()
        .read_book(&mut cursor)
        .expect("Failed to read MOBI");

    assert_eq!(
        decoded.metadata.title.as_deref(),
        Some("MOBI Integration Test")
    );
    assert!(decoded.metadata.authors.iter().any(|a| a == "Test Author"));

    let chapters = decoded.chapters();
    assert!(!chapters.is_empty());

    let all_content: String = chapters.iter().map(|c| c.content.clone()).collect();
    assert!(all_content.contains("First chapter"));
    assert!(all_content.contains("Second chapter"));
}
