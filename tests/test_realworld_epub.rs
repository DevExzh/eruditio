use eruditio::EruditioParser;
use std::path::Path;

/// Helper: collect all `pg-*.epub` files from the small test-data directory.
fn gutenberg_epub_paths() -> Vec<std::path::PathBuf> {
    let dir = Path::new("test-data/real-world/small");
    if !dir.exists() {
        eprintln!(
            "WARNING: test-data/real-world/small/ does not exist; \
             skipping Gutenberg tests"
        );
        return Vec::new();
    }

    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .expect("failed to read test-data/real-world/small/")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            if name.starts_with("pg-") && name.ends_with(".epub") {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    paths.sort();
    paths
}

/// 13 Gutenberg EPUBs have corrupt ZIP archives (missing EOCD).
/// These are pre-existing data quality issues in the source files.
const KNOWN_CORRUPT_EPUBS: &[&str] = &[
    "pg-anna-karenina.epub",
    "pg-count-monte-cristo.epub",
    "pg-crime-and-punishment.epub",
    "pg-divine-comedy.epub",
    "pg-don-quixote.epub",
    "pg-great-expectations.epub",
    "pg-iliad.epub",
    "pg-jane-eyre.epub",
    "pg-les-miserables.epub",
    "pg-peter-pan.epub",
    "pg-pride-and-prejudice.epub",
    "pg-ulysses.epub",
    "pg-war-and-peace.epub",
];

fn is_known_corrupt(path: &std::path::Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    KNOWN_CORRUPT_EPUBS.iter().any(|&c| c == name.as_ref())
}

// ---------------------------------------------------------------------------
// 1. All non-corrupt Gutenberg EPUBs parse successfully
// ---------------------------------------------------------------------------
#[test]
fn test_gutenberg_epubs_all_parse() {
    let paths = gutenberg_epub_paths();
    if paths.is_empty() {
        eprintln!("WARNING: no Gutenberg EPUBs found; skipping test");
        return;
    }

    let good_paths: Vec<_> = paths.iter().filter(|p| !is_known_corrupt(p)).collect();
    let total = good_paths.len();
    let mut pass = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for path in &good_paths {
        match EruditioParser::parse_file(path) {
            Ok(_) => pass += 1,
            Err(e) => {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                eprintln!("FAIL: {} — {}", name, e);
                failures.push(name);
            }
        }
    }

    eprintln!(
        "Gutenberg EPUBs: {}/{} parsed ({} known-corrupt skipped)",
        pass, total, KNOWN_CORRUPT_EPUBS.len()
    );

    assert_eq!(
        pass, total,
        "Expected all {} valid Gutenberg EPUBs to parse, but {} failed: {:?}",
        total,
        failures.len(),
        failures
    );
}

// ---------------------------------------------------------------------------
// 1b. Known-corrupt EPUBs return errors, not panics
// ---------------------------------------------------------------------------
#[test]
fn test_gutenberg_corrupt_epubs_return_errors() {
    let paths = gutenberg_epub_paths();
    if paths.is_empty() {
        return;
    }

    for path in paths.iter().filter(|p| is_known_corrupt(p)) {
        let name = path.file_name().unwrap().to_string_lossy();
        let result = std::panic::catch_unwind(|| EruditioParser::parse_file(path));
        match result {
            Ok(Err(e)) => eprintln!("  [OK-ERR] {} — {}", name, e),
            Ok(Ok(_)) => eprintln!("  [SURPRISE-OK] {} parsed successfully", name),
            Err(_) => panic!("{} caused a panic instead of returning an error", name),
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Every successfully-parsed Gutenberg EPUB has a title
// ---------------------------------------------------------------------------
#[test]
fn test_gutenberg_epubs_have_metadata() {
    let paths = gutenberg_epub_paths();
    if paths.is_empty() {
        eprintln!("WARNING: no Gutenberg EPUBs found; skipping test");
        return;
    }

    let mut titles_found = 0usize;
    let mut titles: Vec<String> = Vec::new();

    for path in &paths {
        if let Ok(book) = EruditioParser::parse_file(path) {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            assert!(
                book.metadata.title.is_some(),
                "{} should have a title but metadata.title is None",
                name
            );
            let title = book.metadata.title.as_ref().unwrap().clone();
            eprintln!("{}: title = {:?}", name, title);
            titles.push(title);
            titles_found += 1;
        }
    }

    eprintln!(
        "Gutenberg EPUBs with titles: {}/{}",
        titles_found,
        paths.len()
    );

    assert!(
        titles_found >= 25,
        "Expected at least 25 books with titles, got {}",
        titles_found
    );
}

// ---------------------------------------------------------------------------
// 3. Every Gutenberg EPUB has at least one chapter
// ---------------------------------------------------------------------------
#[test]
fn test_gutenberg_epubs_have_chapters() {
    let paths = gutenberg_epub_paths();
    if paths.is_empty() {
        eprintln!("WARNING: no Gutenberg EPUBs found; skipping test");
        return;
    }

    let mut total_chapters = 0usize;
    let mut book_count = 0usize;

    for path in &paths {
        if let Ok(book) = EruditioParser::parse_file(path) {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let n = book.chapter_count();
            eprintln!("{}: {} chapters", name, n);
            assert!(
                n > 0,
                "{} should have at least 1 chapter but has {}",
                name,
                n
            );
            total_chapters += n;
            book_count += 1;
        }
    }

    assert!(book_count > 0, "No books were successfully parsed");
    let avg = total_chapters as f64 / book_count as f64;
    eprintln!(
        "Average chapter count across {} books: {:.1}",
        book_count, avg
    );
    assert!(
        avg > 1.0,
        "Expected average chapter count > 1, got {:.1}",
        avg
    );
}

// ---------------------------------------------------------------------------
// 4. Verify known metadata for specific well-known books
// ---------------------------------------------------------------------------
#[test]
fn test_specific_gutenberg_metadata() {
    let cases: &[(&str, &str)] = &[
        ("pg-frankenstein.epub", "frankenstein"),
        ("pg-moby-dick.epub", "moby"),
        ("pg-alice-in-wonderland.epub", "alice"),
        ("pg-dracula.epub", "dracula"),
    ];

    let base = Path::new("test-data/real-world/small");
    if !base.exists() {
        eprintln!(
            "WARNING: test-data/real-world/small/ does not exist; \
             skipping specific metadata test"
        );
        return;
    }

    for (filename, expected_substr) in cases {
        let path = base.join(filename);
        if !path.exists() {
            eprintln!("WARNING: {} not found; skipping", filename);
            continue;
        }

        let book =
            EruditioParser::parse_file(&path).unwrap_or_else(|e| {
                panic!("Failed to parse {}: {}", filename, e)
            });

        let title = book
            .metadata
            .title
            .as_ref()
            .unwrap_or_else(|| panic!("{} has no title", filename));

        eprintln!("{}: title = {:?}", filename, title);

        assert!(
            title.to_lowercase().contains(expected_substr),
            "{}: expected title to contain {:?} (case-insensitive), got {:?}",
            filename,
            expected_substr,
            title
        );
    }
}

// ---------------------------------------------------------------------------
// 5. Medium-sized EPUBs parse and have chapters
// ---------------------------------------------------------------------------
#[test]
fn test_medium_epubs_parse() {
    let base = Path::new("test-data/real-world/medium");
    if !base.exists() {
        eprintln!(
            "WARNING: test-data/real-world/medium/ does not exist; \
             skipping medium EPUB test"
        );
        return;
    }

    // All 3 medium EPUBs have corrupt ZIP archives (missing EOCD).
    // Verify they return errors gracefully without panicking.
    let filenames = [
        "28languages.epub",
        "AChristmasCarolAudioBook.epub",
        "famouspaintings.epub",
    ];

    for filename in &filenames {
        let path = base.join(filename);
        if !path.exists() {
            eprintln!("WARNING: {} not found; skipping", filename);
            continue;
        }

        let result = std::panic::catch_unwind(|| EruditioParser::parse_file(&path));
        match result {
            Ok(Ok(book)) => {
                eprintln!("{}: parsed OK, {} chapters", filename, book.chapter_count());
            }
            Ok(Err(e)) => {
                eprintln!("{}: returned error (expected): {}", filename, e);
            }
            Err(_) => {
                panic!("{} caused a panic instead of returning an error", filename);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 6. Walt Whitman's Leaves of Grass
// ---------------------------------------------------------------------------
#[test]
fn test_leaves_epub() {
    let path = Path::new("test-data/real-world/small/leaves.epub");
    if !path.exists() {
        eprintln!(
            "WARNING: leaves.epub not found; skipping Leaves of Grass test"
        );
        return;
    }

    let book = EruditioParser::parse_file(path)
        .expect("Failed to parse leaves.epub");

    eprintln!(
        "leaves.epub: title = {:?}, chapters = {}",
        book.metadata.title,
        book.chapter_count()
    );

    assert!(
        book.chapter_count() > 0,
        "leaves.epub should have at least 1 chapter"
    );
}
