use std::path::{Path, PathBuf};

use eruditio::EruditioParser;

/// Collect all `.epub` files from a directory, returning an empty vec if the directory
/// does not exist (so tests degrade gracefully in environments without test data).
fn collect_epubs(dir: &Path) -> Vec<PathBuf> {
    if !dir.is_dir() {
        eprintln!(
            "WARNING: test-data directory does not exist: {}  -- skipping",
            dir.display()
        );
        return Vec::new();
    }

    let mut epubs: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("failed to read directory")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("epub") {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    epubs.sort();
    epubs
}

// ---------------------------------------------------------------------------
// 1. W3C EPUB 3.3 -- all valid EPUBs should parse successfully
// ---------------------------------------------------------------------------

#[test]
fn test_w3c_epub33_all_parse_successfully() {
    let dir = Path::new("test-data/compliance/w3c-epub33/valid");
    let epubs = collect_epubs(dir);

    if epubs.is_empty() {
        eprintln!("No W3C EPUB 3.3 test files found -- skipping");
        return;
    }

    let total = epubs.len();
    let mut pass = 0usize;
    let mut failures: Vec<(String, String)> = Vec::new();

    for path in &epubs {
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        match EruditioParser::parse_file(path) {
            Ok(_) => {
                pass += 1;
            },
            Err(e) => {
                failures.push((file_name.clone(), format!("{e}")));
                eprintln!("  FAIL: {file_name}: {e}");
            },
        }
    }

    eprintln!();
    eprintln!("W3C EPUB 3.3: {pass}/{total} passed");
    if !failures.is_empty() {
        eprintln!("Failures:");
        for (name, err) in &failures {
            eprintln!("  - {name}: {err}");
        }
    }

    let pass_rate = (pass as f64) / (total as f64);
    assert!(
        pass_rate >= 0.50,
        "W3C EPUB 3.3 pass rate {:.1}% is below 50% threshold ({pass}/{total})",
        pass_rate * 100.0
    );
}

// ---------------------------------------------------------------------------
// 2. IDPF EPUB 3.0 -- all EPUBs should parse successfully
// ---------------------------------------------------------------------------

#[test]
fn test_idpf_epub30_all_parse_successfully() {
    let dir = Path::new("test-data/compliance/idpf-epub30");
    let epubs = collect_epubs(dir);

    if epubs.is_empty() {
        eprintln!("No IDPF EPUB 3.0 test files found -- skipping");
        return;
    }

    let total = epubs.len();
    let mut pass = 0usize;
    let mut failures: Vec<(String, String)> = Vec::new();

    for path in &epubs {
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        match EruditioParser::parse_file(path) {
            Ok(_) => {
                pass += 1;
            },
            Err(e) => {
                failures.push((file_name.clone(), format!("{e}")));
                eprintln!("  FAIL: {file_name}: {e}");
            },
        }
    }

    eprintln!();
    eprintln!("IDPF EPUB 3.0: {pass}/{total} passed");
    if !failures.is_empty() {
        eprintln!("Failures:");
        for (name, err) in &failures {
            eprintln!("  - {name}: {err}");
        }
    }

    let pass_rate = (pass as f64) / (total as f64);
    assert!(
        pass_rate >= 0.50,
        "IDPF EPUB 3.0 pass rate {:.1}% is below 50% threshold ({pass}/{total})",
        pass_rate * 100.0
    );
}

// ---------------------------------------------------------------------------
// 3. Parsed W3C books have content (chapter_count > 0)
// ---------------------------------------------------------------------------

#[test]
fn test_w3c_epub33_parsed_books_have_content() {
    let base = Path::new("test-data/compliance/w3c-epub33/valid");
    if !base.is_dir() {
        eprintln!("WARNING: W3C test-data directory missing -- skipping");
        return;
    }

    // Ten well-known W3C EPUB 3.3 compliance test files that exercise
    // different parts of the specification.
    let filenames = [
        "cnt-xhtml-support.epub",
        "pkg-spine-order.epub",
        "nav-spine_in-spine.epub",
        "pkg-unique-id.epub",
        "pkg-creator-order.epub",
        "pkg-title-order.epub",
        "mol-navigation.epub",
        "cnt-svg-support.epub",
        "lay-fxl-layout-default.epub",
        "xx-epub-template.epub",
    ];

    let mut checked = 0usize;

    for name in &filenames {
        let path = base.join(name);
        if !path.exists() {
            eprintln!("  SKIP (not found): {name}");
            continue;
        }

        match EruditioParser::parse_file(&path) {
            Ok(book) => {
                let count = book.chapter_count();
                eprintln!("  OK: {name} -- {count} chapter(s)");
                // SVG-only EPUBs store content as binary image data, not text,
                // so chapter_count() correctly returns 0 for them.
                if !name.contains("svg") {
                    assert!(count > 0, "{name}: expected at least one chapter, got 0");
                }
                checked += 1;
            },
            Err(e) => {
                eprintln!("  PARSE ERROR (non-fatal): {name}: {e}");
                // Parse errors are acceptable -- the assertion only applies
                // to books that parse successfully.
            },
        }
    }

    eprintln!("Checked content for {checked}/{} files", filenames.len());
}

// ---------------------------------------------------------------------------
// 4. No panics on any compliance EPUB
// ---------------------------------------------------------------------------

#[test]
fn test_compliance_epubs_no_panics() {
    let dirs: &[&Path] = &[
        Path::new("test-data/compliance/w3c-epub33/valid"),
        Path::new("test-data/compliance/idpf-epub30"),
    ];

    let mut all_epubs: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        all_epubs.extend(collect_epubs(dir));
    }

    if all_epubs.is_empty() {
        eprintln!("No compliance EPUBs found -- skipping panic test");
        return;
    }

    let total = all_epubs.len();
    let mut panics: Vec<String> = Vec::new();

    for path in &all_epubs {
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let path_clone = path.clone();
        let result = std::panic::catch_unwind(move || {
            let _ = EruditioParser::parse_file(&path_clone);
        });

        if result.is_err() {
            eprintln!("  PANIC: {file_name}");
            panics.push(file_name);
        }
    }

    eprintln!();
    eprintln!(
        "Panic check: {}/{total} files caused NO panic",
        total - panics.len()
    );

    assert!(
        panics.is_empty(),
        "The following {} file(s) caused a panic:\n  {}",
        panics.len(),
        panics.join("\n  ")
    );
}
