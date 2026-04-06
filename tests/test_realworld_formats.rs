//! Integration tests for non-EPUB ebook formats using real-world test files
//! from `test-data/real-world/medium/`.

use eruditio::EruditioParser;
use std::path::Path;

/// Helper: resolve a path relative to the project root.
fn test_data_path(relative: &str) -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    Path::new(manifest_dir).join(relative)
}

// ---------------------------------------------------------------------------
// 1. MOBI — known-corrupt file (unrecognized binary), must not panic
// ---------------------------------------------------------------------------

#[test]
fn test_mobi_no_panic() {
    let path = test_data_path("test-data/real-world/medium/sample_marketing_strategies.mobi");
    if !path.exists() {
        eprintln!("[SKIP] MOBI test file not found: {}", path.display());
        return;
    }

    // This file is corrupt (unrecognized binary data). We verify it returns
    // an error gracefully instead of panicking.
    let result = std::panic::catch_unwind(|| EruditioParser::parse_file(&path));
    match result {
        Ok(Ok(book)) => {
            eprintln!(
                "[MOBI] surprisingly parsed OK: {} chapters",
                book.chapter_count()
            );
        },
        Ok(Err(e)) => {
            eprintln!("[MOBI] returned error (expected for corrupt file): {}", e);
        },
        Err(_) => {
            panic!("MOBI file caused a panic instead of returning an error");
        },
    }
}

// ---------------------------------------------------------------------------
// 2. CBZ — Elf_Receiver is known-good; sample_comic_book uses unusual compression
// ---------------------------------------------------------------------------

#[test]
fn test_cbz_parse() {
    // Known-good CBZ
    let good_path =
        test_data_path("test-data/real-world/medium/Elf_Receiver_Radio-Craft_August_1936.cbz");
    if good_path.exists() {
        let book = EruditioParser::parse_file(&good_path)
            .expect("Failed to parse known-good CBZ (Elf_Receiver)");
        assert!(
            book.chapter_count() >= 1,
            "Elf_Receiver CBZ should have at least 1 chapter, got {}",
            book.chapter_count()
        );
        eprintln!("[CBZ] Elf_Receiver -> {} chapters", book.chapter_count());
    } else {
        eprintln!("[SKIP] Elf_Receiver CBZ not found");
    }

    // sample_comic_book.cbz uses unusual ZIP compression (0x4c45), may fail
    let suspect_path = test_data_path("test-data/real-world/medium/sample_comic_book.cbz");
    if suspect_path.exists() {
        match EruditioParser::parse_file(&suspect_path) {
            Ok(book) => eprintln!(
                "[CBZ] sample_comic_book -> {} chapters",
                book.chapter_count()
            ),
            Err(e) => eprintln!(
                "[CBZ] sample_comic_book returned error (unusual compression): {}",
                e
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// 3. CBR — file is actually a ZIP archive, not RAR; must not panic
// ---------------------------------------------------------------------------

#[test]
fn test_cbr_no_panic() {
    let path = test_data_path("test-data/real-world/medium/test_large_12pages.cbr");
    if !path.exists() {
        eprintln!("[SKIP] CBR test file not found: {}", path.display());
        return;
    }

    // This .cbr file is actually a ZIP archive (mislabeled). Verify graceful handling.
    let result = std::panic::catch_unwind(|| EruditioParser::parse_file(&path));
    match result {
        Ok(Ok(book)) => {
            eprintln!("[CBR] parsed OK: {} chapters", book.chapter_count());
        },
        Ok(Err(e)) => {
            eprintln!("[CBR] returned error (expected for mislabeled file): {}", e);
        },
        Err(_) => {
            panic!("CBR file caused a panic instead of returning an error");
        },
    }
}

// ---------------------------------------------------------------------------
// 4. CB7 — file is actually gzip, not 7z; must not panic
// ---------------------------------------------------------------------------

#[test]
fn test_cb7_no_panic() {
    let path = test_data_path("test-data/real-world/medium/test_mixed_formats.cb7");
    if !path.exists() {
        eprintln!("[SKIP] CB7 test file not found: {}", path.display());
        return;
    }

    // This .cb7 file is actually gzip-compressed (mislabeled). Verify graceful handling.
    let result = std::panic::catch_unwind(|| EruditioParser::parse_file(&path));
    match result {
        Ok(Ok(book)) => {
            eprintln!("[CB7] parsed OK: {} chapters", book.chapter_count());
        },
        Ok(Err(e)) => {
            eprintln!("[CB7] returned error (expected for mislabeled file): {}", e);
        },
        Err(_) => {
            panic!("CB7 file caused a panic instead of returning an error");
        },
    }
}

// ---------------------------------------------------------------------------
// 5. CBC — files may be missing comics.txt manifest; must not panic
// ---------------------------------------------------------------------------

#[test]
fn test_cbc_no_panic() {
    let files = [
        "test-data/real-world/medium/test_collection_large.cbc",
        "test-data/real-world/medium/test_large_20pages.cbc",
    ];

    for rel in &files {
        let path = test_data_path(rel);
        if !path.exists() {
            eprintln!("[SKIP] CBC test file not found: {}", path.display());
            continue;
        }

        let result = std::panic::catch_unwind(|| EruditioParser::parse_file(&path));
        match result {
            Ok(Ok(book)) => {
                eprintln!("[CBC] {} -> {} chapters", rel, book.chapter_count());
            },
            Ok(Err(e)) => {
                eprintln!("[CBC] {} returned error: {}", rel, e);
            },
            Err(_) => {
                panic!(
                    "CBC file {} caused a panic instead of returning an error",
                    rel
                );
            },
        }
    }
}

// ---------------------------------------------------------------------------
// 6. Catch-all: ensure no file in medium/ causes a panic
// ---------------------------------------------------------------------------

#[test]
fn test_all_medium_formats_no_panics() {
    let dir = test_data_path("test-data/real-world/medium");
    if !dir.is_dir() {
        eprintln!("[SKIP] test-data/real-world/medium/ directory not found");
        return;
    }

    let entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("Failed to read medium directory")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| !ext.is_empty()))
        .collect();

    let total = entries.len();
    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut panics = 0usize;

    for entry in &entries {
        let path = entry.path();
        let filename = path.file_name().unwrap().to_string_lossy().to_string();

        let result = std::panic::catch_unwind(|| EruditioParser::parse_file(&path));

        match result {
            Ok(Ok(book)) => {
                eprintln!(
                    "  [OK]    {} -> {} chapters",
                    filename,
                    book.chapter_count()
                );
                pass += 1;
            },
            Ok(Err(err)) => {
                eprintln!("  [ERR]   {} -> {}", filename, err);
                fail += 1;
            },
            Err(_) => {
                eprintln!("  [PANIC] {}", filename);
                panics += 1;
            },
        }
    }

    eprintln!(
        "\nSummary: {} parsed, {} errors, {} panics out of {}",
        pass, fail, panics, total
    );

    assert_eq!(
        panics, 0,
        "Some files caused panics! {} panics out of {} files",
        panics, total
    );
}
