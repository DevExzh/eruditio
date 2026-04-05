use eruditio::domain::{Book, Chapter};
use eruditio::domain::{FormatReader, FormatWriter};
use eruditio::formats::{MobiReader, MobiWriter};
use std::io::Cursor;
use std::path::Path;

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

/// Integration test for KF8/AZW3 file: verifies that kindle:embed and kindle:flow
/// references are resolved and flow resources are extracted.
#[test]
fn kf8_kindle_references_resolved() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test-data/real-world/large/pride_prejudice_kf8.mobi");
    if !path.exists() {
        eprintln!("[SKIP] KF8 test file not found: {}", path.display());
        return;
    }

    let data = std::fs::read(&path).expect("Failed to read KF8 file");
    let mut cursor = Cursor::new(data);
    let book = MobiReader::new()
        .read_book(&mut cursor)
        .expect("Failed to read KF8 file");

    // Verify chapters exist.
    let chapters = book.chapters();
    assert!(!chapters.is_empty(), "KF8 file should have chapters");

    // Check that no kindle:embed or kindle:flow references remain in chapter content.
    // Note: kindle:pos:fid references (internal position links) are a separate
    // concern and are not resolved by the embed/flow resolver.
    let mut unresolved_embed_flow = 0;
    for ch in &chapters {
        unresolved_embed_flow += ch.content.matches("kindle:embed:").count();
        unresolved_embed_flow += ch.content.matches("kindle:flow:").count();
    }
    assert_eq!(
        unresolved_embed_flow, 0,
        "All kindle:embed and kindle:flow references should be resolved, but {} remain",
        unresolved_embed_flow
    );

    // Verify image resources exist.
    let image_resources: Vec<_> = book
        .manifest
        .iter()
        .filter(|item| item.media_type.starts_with("image/"))
        .collect();
    assert!(
        !image_resources.is_empty(),
        "KF8 file should have image resources"
    );

    // Verify flow resources (CSS) exist.
    let css_resources: Vec<_> = book
        .manifest
        .iter()
        .filter(|item| item.media_type == "text/css")
        .collect();
    assert!(
        !css_resources.is_empty(),
        "KF8 file should have CSS flow resources"
    );
}
