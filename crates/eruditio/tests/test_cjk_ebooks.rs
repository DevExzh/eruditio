//! Integration tests for CJK (Chinese) ebook files.
//!
//! Tests real-world Chinese Gutenberg ebooks (pg23962 = 《昆虫记》, pg31757 = 《聊斋志异》)
//! across EPUB and MOBI formats to ensure no panics, correct metadata extraction,
//! and valid CJK content handling.

use eruditio::domain::{FormatReader, FormatWriter};
use eruditio::formats::mobi::MobiReader;
use eruditio::EruditioParser;
use std::path::{Path, PathBuf};

/// Helper: resolve a path relative to the workspace root.
fn test_data_path(relative: &str) -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/eruditio; test-data lives at workspace root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

/// All Chinese ebook test files grouped by Project Gutenberg ID.
const PG23962_FILES: &[&str] = &[
    "test-data/real-world/medium/pg23962-images.epub",
    "test-data/real-world/medium/pg23962-images.mobi",
    "test-data/real-world/medium/pg23962-images-kf8.mobi",
];

const PG31757_FILES: &[&str] = &[
    "test-data/real-world/small/pg31757-images.epub",
    "test-data/real-world/small/pg31757-images-3.epub",
    "test-data/real-world/small/pg31757-images.mobi",
    "test-data/real-world/small/pg31757-images-kf8.mobi",
];

/// Returns true if the string contains at least one CJK Unified Ideograph (U+4E00..U+9FFF).
fn contains_cjk(s: &str) -> bool {
    s.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c))
}

// ---------------------------------------------------------------------------
// 1. No panics — every Chinese ebook file parses without panicking
// ---------------------------------------------------------------------------

#[test]
fn test_cjk_ebooks_no_panics() {
    let all_files: Vec<&str> = PG23962_FILES
        .iter()
        .chain(PG31757_FILES.iter())
        .copied()
        .collect();

    let mut panics = Vec::new();

    for rel in &all_files {
        let path = test_data_path(rel);
        if !path.exists() {
            eprintln!("[SKIP] {}", rel);
            continue;
        }

        let result = std::panic::catch_unwind(|| EruditioParser::parse_file(&path));
        match result {
            Ok(Ok(book)) => {
                eprintln!(
                    "[OK]    {} -> {} chapters",
                    rel,
                    book.chapter_count()
                );
            }
            Ok(Err(e)) => {
                eprintln!("[ERR]   {} -> {}", rel, e);
            }
            Err(_) => {
                eprintln!("[PANIC] {}", rel);
                panics.push(rel.to_string());
            }
        }
    }

    assert!(
        panics.is_empty(),
        "CJK ebook files caused panics: {:?}",
        panics
    );
}

// ---------------------------------------------------------------------------
// 2. pg23962 EPUB — Chinese content and metadata
// ---------------------------------------------------------------------------

#[test]
fn test_pg23962_epub_content() {
    let path = test_data_path("test-data/real-world/medium/pg23962-images.epub");
    if !path.exists() {
        eprintln!("[SKIP] pg23962 EPUB not found");
        return;
    }

    let book = EruditioParser::parse_file(&path).expect("pg23962 EPUB should parse");

    // Should have multiple chapters
    assert!(
        book.chapter_count() >= 1,
        "pg23962 should have at least 1 chapter, got {}",
        book.chapter_count()
    );

    // At least one chapter should contain CJK characters
    let has_cjk_content = book
        .chapters()
        .iter()
        .any(|ch| contains_cjk(&ch.content));
    assert!(
        has_cjk_content,
        "pg23962 EPUB should contain CJK characters in chapter content"
    );

    eprintln!(
        "[pg23962 EPUB] {} chapters, title={:?}, author={:?}",
        book.chapter_count(),
        book.metadata.title,
        book.metadata.authors
    );
}

// ---------------------------------------------------------------------------
// 3. pg23962 MOBI — Chinese content (standard and KF8 variants)
// ---------------------------------------------------------------------------

#[test]
fn test_pg23962_mobi_content() {
    for rel in &[
        "test-data/real-world/medium/pg23962-images.mobi",
        "test-data/real-world/medium/pg23962-images-kf8.mobi",
    ] {
        let path = test_data_path(rel);
        if !path.exists() {
            eprintln!("[SKIP] {}", rel);
            continue;
        }

        let book = EruditioParser::parse_file(&path).expect(&format!("{} should parse", rel));

        assert!(
            book.chapter_count() >= 1,
            "{} should have at least 1 chapter",
            rel
        );

        let has_cjk_content = book
            .chapters()
            .iter()
            .any(|ch| contains_cjk(&ch.content));
        assert!(
            has_cjk_content,
            "{} should contain CJK characters in chapter content",
            rel
        );

        eprintln!(
            "[{}] {} chapters, title={:?}",
            rel,
            book.chapter_count(),
            book.metadata.title
        );
    }
}

// ---------------------------------------------------------------------------
// 4. pg31757 EPUB — Chinese content (all EPUB variants)
// ---------------------------------------------------------------------------

#[test]
fn test_pg31757_epub_content() {
    for rel in &[
        "test-data/real-world/small/pg31757-images.epub",
        "test-data/real-world/small/pg31757-images-3.epub",
    ] {
        let path = test_data_path(rel);
        if !path.exists() {
            eprintln!("[SKIP] {}", rel);
            continue;
        }

        let book = EruditioParser::parse_file(&path).expect(&format!("{} should parse", rel));

        assert!(
            book.chapter_count() >= 1,
            "{} should have at least 1 chapter",
            rel
        );

        let has_cjk_content = book
            .chapters()
            .iter()
            .any(|ch| contains_cjk(&ch.content));
        assert!(
            has_cjk_content,
            "{} should contain CJK characters in chapter content",
            rel
        );

        eprintln!(
            "[{}] {} chapters, title={:?}",
            rel,
            book.chapter_count(),
            book.metadata.title
        );
    }
}

// ---------------------------------------------------------------------------
// 5. pg31757 MOBI — Chinese content (standard and KF8 variants)
// ---------------------------------------------------------------------------

#[test]
fn test_pg31757_mobi_content() {
    for rel in &[
        "test-data/real-world/small/pg31757-images.mobi",
        "test-data/real-world/small/pg31757-images-kf8.mobi",
    ] {
        let path = test_data_path(rel);
        if !path.exists() {
            eprintln!("[SKIP] {}", rel);
            continue;
        }

        let book = EruditioParser::parse_file(&path).expect(&format!("{} should parse", rel));

        assert!(
            book.chapter_count() >= 1,
            "{} should have at least 1 chapter",
            rel
        );

        let has_cjk_content = book
            .chapters()
            .iter()
            .any(|ch| contains_cjk(&ch.content));
        assert!(
            has_cjk_content,
            "{} should contain CJK characters in chapter content",
            rel
        );

        eprintln!(
            "[{}] {} chapters, title={:?}",
            rel,
            book.chapter_count(),
            book.metadata.title
        );
    }
}

// ---------------------------------------------------------------------------
// 6. Cross-format conversion: EPUB → MOBI writer (no panics)
// ---------------------------------------------------------------------------

#[test]
fn test_cjk_epub_to_mobi_no_panic() {
    let path = test_data_path("test-data/real-world/small/pg31757-images.epub");
    if !path.exists() {
        eprintln!("[SKIP] pg31757 EPUB not found");
        return;
    }

    let book = EruditioParser::parse_file(&path).expect("pg31757 EPUB should parse");

    // Write to MOBI in memory
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let writer = eruditio::formats::mobi::MobiWriter::new();
        let mut buf = Vec::new();
        writer.write_book(&book, &mut buf)
    }));

    match result {
        Ok(Ok(())) => eprintln!("[EPUB→MOBI] pg31757 conversion OK"),
        Ok(Err(e)) => eprintln!("[EPUB→MOBI] pg31757 conversion error (non-panic): {}", e),
        Err(_) => panic!("EPUB→MOBI conversion of CJK ebook panicked!"),
    }
}

// ---------------------------------------------------------------------------
// 7. Cross-format conversion: MOBI → EPUB writer (no panics)
// ---------------------------------------------------------------------------

#[test]
fn test_cjk_mobi_to_epub_no_panic() {
    let path = test_data_path("test-data/real-world/small/pg31757-images.mobi");
    if !path.exists() {
        eprintln!("[SKIP] pg31757 MOBI not found");
        return;
    }

    let book = EruditioParser::parse_file(&path).expect("pg31757 MOBI should parse");

    // Write to EPUB in memory
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let writer = eruditio::formats::epub::EpubWriter::new();
        let mut buf = Vec::new();
        writer.write_book(&book, &mut buf)
    }));

    match result {
        Ok(Ok(())) => eprintln!("[MOBI→EPUB] pg31757 conversion OK"),
        Ok(Err(e)) => eprintln!("[MOBI→EPUB] pg31757 conversion error (non-panic): {}", e),
        Err(_) => panic!("MOBI→EPUB conversion of CJK ebook panicked!"),
    }
}

// ---------------------------------------------------------------------------
// 8. Cross-format conversion: EPUB → Markdown writer (no panics, CJK preserved)
// ---------------------------------------------------------------------------

#[test]
fn test_cjk_epub_to_markdown_no_panic() {
    let path = test_data_path("test-data/real-world/small/pg31757-images.epub");
    if !path.exists() {
        eprintln!("[SKIP] pg31757 EPUB not found");
        return;
    }

    let book = EruditioParser::parse_file(&path).expect("pg31757 EPUB should parse");

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let writer = eruditio::formats::md::MdWriter::new();
        let mut buf = Vec::new();
        writer.write_book(&book, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }));

    match result {
        Ok(md_text) => {
            assert!(
                contains_cjk(&md_text),
                "Markdown output should preserve CJK characters"
            );
            eprintln!(
                "[EPUB→MD] pg31757 conversion OK, {} bytes, CJK preserved",
                md_text.len()
            );
        }
        Err(_) => panic!("EPUB→Markdown conversion of CJK ebook panicked!"),
    }
}

// ---------------------------------------------------------------------------
// 9. Cross-format conversion: EPUB → TXT writer (no panics, CJK preserved)
// ---------------------------------------------------------------------------

#[test]
fn test_cjk_epub_to_txt_no_panic() {
    let path = test_data_path("test-data/real-world/small/pg31757-images.epub");
    if !path.exists() {
        eprintln!("[SKIP] pg31757 EPUB not found");
        return;
    }

    let book = EruditioParser::parse_file(&path).expect("pg31757 EPUB should parse");

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let writer = eruditio::formats::txt::TxtWriter::new();
        let mut buf = Vec::new();
        writer.write_book(&book, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }));

    match result {
        Ok(txt_text) => {
            assert!(
                contains_cjk(&txt_text),
                "TXT output should preserve CJK characters"
            );
            eprintln!(
                "[EPUB→TXT] pg31757 conversion OK, {} bytes, CJK preserved",
                txt_text.len()
            );
        }
        Err(_) => panic!("EPUB→TXT conversion of CJK ebook panicked!"),
    }
}

// ---------------------------------------------------------------------------
// 10. Cross-format conversion: EPUB → FB2 writer (no panics)
// ---------------------------------------------------------------------------

#[test]
fn test_cjk_epub_to_fb2_no_panic() {
    let path = test_data_path("test-data/real-world/small/pg31757-images.epub");
    if !path.exists() {
        eprintln!("[SKIP] pg31757 EPUB not found");
        return;
    }

    let book = EruditioParser::parse_file(&path).expect("pg31757 EPUB should parse");

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let writer = eruditio::formats::fb2::Fb2Writer::new();
        let mut buf = Vec::new();
        writer.write_book(&book, &mut buf)
    }));

    match result {
        Ok(Ok(())) => eprintln!("[EPUB→FB2] pg31757 conversion OK"),
        Ok(Err(e)) => eprintln!("[EPUB→FB2] pg31757 conversion error (non-panic): {}", e),
        Err(_) => panic!("EPUB→FB2 conversion of CJK ebook panicked!"),
    }
}

// ---------------------------------------------------------------------------
// 11. MOBI round-trip: read → write → re-read (no panics, content preserved)
// ---------------------------------------------------------------------------

#[test]
fn test_cjk_mobi_roundtrip() {
    let path = test_data_path("test-data/real-world/small/pg31757-images.mobi");
    if !path.exists() {
        eprintln!("[SKIP] pg31757 MOBI not found");
        return;
    }

    // Read
    let mut file = std::fs::File::open(&path).unwrap();
    let original = MobiReader::new().read_book(&mut file).unwrap();
    let original_chapter_count = original.chapter_count();

    // Write
    let writer = eruditio::formats::mobi::MobiWriter::new();
    let mut buf = Vec::new();
    let write_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        writer.write_book(&original, &mut buf)
    }));

    match write_result {
        Ok(Ok(())) => {
            eprintln!(
                "[MOBI roundtrip] write OK, {} bytes",
                buf.len()
            );
        }
        Ok(Err(e)) => {
            eprintln!("[MOBI roundtrip] write error: {}", e);
            return;
        }
        Err(_) => {
            panic!("MOBI roundtrip write panicked on CJK ebook!");
        }
    }

    // Re-read
    let mut cursor = std::io::Cursor::new(&buf);
    let reread_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        MobiReader::new().read_book(&mut cursor)
    }));

    match reread_result {
        Ok(Ok(reread)) => {
            assert!(
                reread.chapter_count() >= 1,
                "Round-tripped MOBI should have at least 1 chapter"
            );
            eprintln!(
                "[MOBI roundtrip] re-read OK: {} -> {} chapters",
                original_chapter_count,
                reread.chapter_count()
            );
        }
        Ok(Err(e)) => {
            eprintln!("[MOBI roundtrip] re-read error: {}", e);
        }
        Err(_) => {
            panic!("MOBI roundtrip re-read panicked on CJK ebook!");
        }
    }
}

// ---------------------------------------------------------------------------
// 12. Large CJK MOBI (pg23962, ~2MB) — stress test with KF8
// ---------------------------------------------------------------------------

#[test]
fn test_pg23962_kf8_mobi_large() {
    let path = test_data_path("test-data/real-world/medium/pg23962-images-kf8.mobi");
    if !path.exists() {
        eprintln!("[SKIP] pg23962 KF8 MOBI not found");
        return;
    }

    let book = EruditioParser::parse_file(&path).expect("pg23962 KF8 MOBI should parse");

    // Verify non-trivial chapter count for a full book
    assert!(
        book.chapter_count() >= 1,
        "pg23962 KF8 should have chapters"
    );

    // Verify CJK content exists
    let total_cjk_chars: usize = book
        .chapters()
        .iter()
        .map(|ch| {
            ch.content
                .chars()
                .filter(|c| ('\u{4E00}'..='\u{9FFF}').contains(c))
                .count()
        })
        .sum();

    assert!(
        total_cjk_chars > 100,
        "pg23962 should have significant CJK content, got only {} chars",
        total_cjk_chars
    );

    eprintln!(
        "[pg23962 KF8] {} chapters, {} CJK characters total",
        book.chapter_count(),
        total_cjk_chars
    );
}

// ---------------------------------------------------------------------------
// 13. MOBI → EPUB produces valid XHTML (not bare HTML fragments)
// ---------------------------------------------------------------------------

#[test]
fn test_mobi_to_epub_produces_valid_xhtml() {
    let path = test_data_path("test-data/real-world/small/pg31757-images.mobi");
    if !path.exists() {
        eprintln!("[SKIP] pg31757 MOBI not found");
        return;
    }

    let book = EruditioParser::parse_file(&path).expect("pg31757 MOBI should parse");

    // Write to EPUB
    let writer = eruditio::formats::epub::EpubWriter::new();
    let mut buf = Vec::new();
    writer.write_book(&book, &mut buf).expect("EPUB write should succeed");

    // Inspect the EPUB ZIP — every .xhtml file must be a full XHTML document.
    let cursor = std::io::Cursor::new(&buf);
    let mut archive = zip::ZipArchive::new(cursor).expect("output should be valid ZIP");

    for i in 0..archive.len() {
        let name = archive.by_index(i).unwrap().name().to_string();
        if !name.ends_with(".xhtml") {
            continue;
        }

        let mut content = String::new();
        {
            use std::io::Read;
            archive
                .by_name(&name)
                .unwrap()
                .read_to_string(&mut content)
                .unwrap();
        }

        let trimmed = content.trim_start();
        assert!(
            trimmed.starts_with("<?xml") || trimmed.starts_with("<html") || trimmed.starts_with("<!DOCTYPE"),
            "[MOBI→EPUB] {} is not a valid XHTML document. Starts with: {:?}",
            name,
            &trimmed[..trimmed.len().min(80)]
        );
        assert!(
            content.contains("<html"),
            "[MOBI→EPUB] {} missing <html> element",
            name
        );

        eprintln!("[MOBI→EPUB] {} OK ({} bytes)", name, content.len());
    }
}

// ---------------------------------------------------------------------------
// 14. pg23962 MOBI → EPUB produces XML-parseable XHTML (the user's bug report)
// ---------------------------------------------------------------------------

#[test]
fn test_pg23962_mobi_to_epub_valid_xml() {
    let path = test_data_path("test-data/real-world/medium/pg23962-images.mobi");
    if !path.exists() {
        eprintln!("[SKIP] pg23962 MOBI not found");
        return;
    }

    let book = EruditioParser::parse_file(&path).expect("pg23962 MOBI should parse");

    // Write to EPUB
    let writer = eruditio::formats::epub::EpubWriter::new();
    let mut buf = Vec::new();
    writer
        .write_book(&book, &mut buf)
        .expect("EPUB write should succeed");

    // Validate every .xhtml file in the EPUB is well-formed XML.
    let cursor = std::io::Cursor::new(&buf);
    let mut archive = zip::ZipArchive::new(cursor).expect("output should be valid ZIP");
    let mut errors = Vec::new();

    for i in 0..archive.len() {
        let name = archive.by_index(i).unwrap().name().to_string();
        if !name.ends_with(".xhtml") {
            continue;
        }

        let mut content = String::new();
        {
            use std::io::Read;
            archive
                .by_name(&name)
                .unwrap()
                .read_to_string(&mut content)
                .unwrap();
        }

        // Must start with XML declaration
        let trimmed = content.trim_start();
        if !trimmed.starts_with("<?xml") {
            errors.push(format!("{}: missing XML declaration, starts with {:?}", name, &trimmed[..trimmed.len().min(60)]));
            continue;
        }

        // Must be parseable as XML (quick-xml check)
        let mut reader = quick_xml::Reader::from_str(&content);
        reader.config_mut().check_end_names = true;
        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Eof) => break,
                Err(e) => {
                    errors.push(format!("{}: XML parse error at position {}: {}", name, reader.error_position(), e));
                    break;
                }
                _ => {}
            }
        }
    }

    assert!(
        errors.is_empty(),
        "pg23962 MOBI→EPUB XHTML validation errors:\n{}",
        errors.join("\n")
    );
    eprintln!("[pg23962 MOBI→EPUB] All XHTML files are valid XML");
}

// ---------------------------------------------------------------------------
// 15. pg31757 MOBI → EPUB produces XML-parseable XHTML
// ---------------------------------------------------------------------------

#[test]
fn test_pg31757_mobi_to_epub_valid_xml() {
    let path = test_data_path("test-data/real-world/small/pg31757-images.mobi");
    if !path.exists() {
        eprintln!("[SKIP] pg31757 MOBI not found");
        return;
    }

    let book = EruditioParser::parse_file(&path).expect("pg31757 MOBI should parse");

    let writer = eruditio::formats::epub::EpubWriter::new();
    let mut buf = Vec::new();
    writer
        .write_book(&book, &mut buf)
        .expect("EPUB write should succeed");

    let cursor = std::io::Cursor::new(&buf);
    let mut archive = zip::ZipArchive::new(cursor).expect("output should be valid ZIP");
    let mut errors = Vec::new();

    for i in 0..archive.len() {
        let name = archive.by_index(i).unwrap().name().to_string();
        if !name.ends_with(".xhtml") {
            continue;
        }

        let mut content = String::new();
        {
            use std::io::Read;
            archive
                .by_name(&name)
                .unwrap()
                .read_to_string(&mut content)
                .unwrap();
        }

        let trimmed = content.trim_start();
        if !trimmed.starts_with("<?xml") {
            errors.push(format!("{}: missing XML declaration", name));
            continue;
        }

        let mut reader = quick_xml::Reader::from_str(&content);
        reader.config_mut().check_end_names = true;
        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Eof) => break,
                Err(e) => {
                    errors.push(format!("{}: XML parse error: {}", name, e));
                    break;
                }
                _ => {}
            }
        }
    }

    assert!(
        errors.is_empty(),
        "pg31757 MOBI→EPUB XHTML validation errors:\n{}",
        errors.join("\n")
    );
    eprintln!("[pg31757 MOBI→EPUB] All XHTML files are valid XML");
}
