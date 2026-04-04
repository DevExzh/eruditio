use eruditio::EruditioParser;
use std::path::{Path, PathBuf};

/// Return the path to the real-world small test-data directory.
fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/real-world/small")
}

/// All expected HTML filenames in the test-data directory.
fn html_filenames() -> Vec<&'static str> {
    vec![
        "alice_wonderland.html",
        "emma.html",
        "frankenstein.html",
        "metamorphosis.html",
        "picture_of_dorian_gray.html",
        "pride_prejudice.html",
        "strange_case_jekyll_hyde.html",
        "tale_of_two_cities.html",
    ]
}

/// Collect all `.html` files actually present in the test-data directory.
fn discover_html_files() -> Vec<PathBuf> {
    let dir = test_data_dir();
    if !dir.exists() {
        eprintln!("test-data directory not found: {}", dir.display());
        return Vec::new();
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("failed to read test-data directory")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("html") {
                Some(path)
            } else {
                None
            }
        })
        .collect();
    files.sort();
    files
}

/// Helper: join all chapter content for a parsed book.
fn all_content(book: &eruditio::Book) -> String {
    book.chapters()
        .iter()
        .map(|c| &c.content as &str)
        .collect::<Vec<_>>()
        .join("")
}

// ---------------------------------------------------------------------------
// 1. Every HTML file in the directory must parse successfully.
// ---------------------------------------------------------------------------
#[test]
fn test_all_html_files_parse() {
    let files = discover_html_files();
    assert!(
        !files.is_empty(),
        "No HTML files found in test-data directory"
    );

    let total = files.len();
    let mut pass = 0usize;

    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy();
        match EruditioParser::parse_file(path) {
            Ok(_) => {
                pass += 1;
                eprintln!("[PASS] {}", name);
            },
            Err(e) => {
                eprintln!("[FAIL] {} — {}", name, e);
            },
        }
    }

    eprintln!("HTML files: {}/{} parsed", pass, total);
    assert_eq!(pass, total, "Not all HTML files parsed successfully");
}

// ---------------------------------------------------------------------------
// 2. Every parsed book must produce at least one non-empty chapter.
// ---------------------------------------------------------------------------
#[test]
fn test_html_produces_content() {
    let files = discover_html_files();
    assert!(
        !files.is_empty(),
        "No HTML files found in test-data directory"
    );

    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy();
        let book = EruditioParser::parse_file(path)
            .unwrap_or_else(|e| panic!("Failed to parse {}: {}", name, e));

        let count = book.chapter_count();
        eprintln!("{}: {} chapter(s)", name, count);

        assert!(count > 0, "{} produced zero chapters", name);

        let chapters = book.chapters();
        assert!(
            !chapters.is_empty(),
            "{}: chapters() returned an empty vec despite chapter_count > 0",
            name
        );
    }
}

// ---------------------------------------------------------------------------
// 3. Alice in Wonderland must have substantial content.
// ---------------------------------------------------------------------------
#[test]
fn test_html_content_not_empty() {
    let path = test_data_dir().join("alice_wonderland.html");
    if !path.exists() {
        eprintln!("Skipping test — file not found: {}", path.display());
        return;
    }

    let book = EruditioParser::parse_file(&path).expect("Failed to parse alice_wonderland.html");
    let chapters = book.chapters();

    assert!(
        !chapters.is_empty(),
        "alice_wonderland.html has no chapters"
    );

    let first = &chapters[0];
    assert!(!first.content.is_empty(), "First chapter content is empty");
    assert!(
        first.content.len() > 100,
        "First chapter content is suspiciously short ({} chars)",
        first.content.len()
    );

    eprintln!(
        "alice_wonderland.html — first chapter length: {} chars",
        first.content.len()
    );
}

// ---------------------------------------------------------------------------
// 4. Spot-check specific content in well-known novels.
// ---------------------------------------------------------------------------
#[test]
fn test_html_specific_content() {
    let dir = test_data_dir();

    // Pride and Prejudice — must mention "Bennet"
    {
        let path = dir.join("pride_prejudice.html");
        if path.exists() {
            let book =
                EruditioParser::parse_file(&path).expect("Failed to parse pride_prejudice.html");
            let content = all_content(&book);
            assert!(
                content.contains("Bennet"),
                "pride_prejudice.html: expected content to contain 'Bennet'"
            );
            eprintln!("pride_prejudice.html — found 'Bennet'");
        } else {
            eprintln!("Skipping pride_prejudice.html — file not found");
        }
    }

    // Frankenstein — must mention "creature" or "monster"
    {
        let path = dir.join("frankenstein.html");
        if path.exists() {
            let book =
                EruditioParser::parse_file(&path).expect("Failed to parse frankenstein.html");
            let content = all_content(&book);
            assert!(
                content.contains("creature") || content.contains("monster"),
                "frankenstein.html: expected content to contain 'creature' or 'monster'"
            );
            eprintln!("frankenstein.html — found expected keyword");
        } else {
            eprintln!("Skipping frankenstein.html — file not found");
        }
    }

    // The Metamorphosis — must mention "Gregor" or "Samsa"
    {
        let path = dir.join("metamorphosis.html");
        if path.exists() {
            let book =
                EruditioParser::parse_file(&path).expect("Failed to parse metamorphosis.html");
            let content = all_content(&book);
            assert!(
                content.contains("Gregor") || content.contains("Samsa"),
                "metamorphosis.html: expected content to contain 'Gregor' or 'Samsa'"
            );
            eprintln!("metamorphosis.html — found expected keyword");
        } else {
            eprintln!("Skipping metamorphosis.html — file not found");
        }
    }
}

// ---------------------------------------------------------------------------
// 5. Parsing every file must not panic.
// ---------------------------------------------------------------------------
#[test]
fn test_html_no_panics() {
    let filenames = html_filenames();
    let dir = test_data_dir();
    let mut panic_count = 0usize;

    for name in &filenames {
        let path = dir.join(name);
        if !path.exists() {
            eprintln!("Skipping {} — file not found", name);
            continue;
        }

        let result = std::panic::catch_unwind(|| {
            let _ = EruditioParser::parse_file(&path);
        });

        if result.is_err() {
            eprintln!("[PANIC] {}", name);
            panic_count += 1;
        } else {
            eprintln!("[OK]    {}", name);
        }
    }

    assert_eq!(
        panic_count, 0,
        "{} HTML file(s) caused a panic",
        panic_count
    );
}
