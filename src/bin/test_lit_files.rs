//! Test harness for reading LIT files from assets/lit/.
//!
//! Usage: cargo run --bin test_lit_files

use eruditio::domain::FormatReader;
use eruditio::formats::lit::LitReader;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

fn main() {
    let lit_dir = Path::new("assets/lit");
    if !lit_dir.exists() {
        eprintln!("assets/lit/ directory not found");
        std::process::exit(1);
    }

    let mut entries: Vec<_> = fs::read_dir(lit_dir)
        .expect("read dir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext == "lit")
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let reader = LitReader::new();
    let mut success = 0u32;
    let mut fail = 0u32;

    println!("{}", "=".repeat(80));
    println!("LIT File Test Report (Rust eruditio)");
    println!("{}", "=".repeat(80));

    for entry in &entries {
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy();
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

        // Check magic bytes
        let data = match fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                println!("\n--- {} ({} bytes) ---", name, size);
                println!("  ERROR: Could not read file: {}", e);
                fail += 1;
                continue;
            }
        };

        let magic = if data.len() >= 8 {
            String::from_utf8_lossy(&data[0..8]).to_string()
        } else {
            format!("<{} bytes>", data.len())
        };

        println!("\n--- {} ({} bytes, magic: {:?}) ---", name, size, magic);

        // Skip non-ITOLITLS files
        if data.len() < 8 || &data[0..8] != b"ITOLITLS" {
            println!("  SKIP: Not a valid ITOLITLS file");
            continue;
        }

        let start = Instant::now();
        let mut cursor = Cursor::new(data);
        match reader.read_book(&mut cursor) {
            Ok(book) => {
                let elapsed = start.elapsed();
                success += 1;
                println!("  OK ({:.1}ms)", elapsed.as_secs_f64() * 1000.0);
                println!(
                    "  Title:    {}",
                    book.metadata.title.as_deref().unwrap_or("<none>")
                );
                println!(
                    "  Authors:  {}",
                    if book.metadata.authors.is_empty() {
                        "<none>".to_string()
                    } else {
                        book.metadata.authors.join(", ")
                    }
                );
                println!(
                    "  Language: {}",
                    book.metadata.language.as_deref().unwrap_or("<none>")
                );
                println!("  Chapters: {}", book.chapters().len());
                println!("  Resources: {}", book.resources().len());

                // Show first chapter preview
                if let Some(ch) = book.chapters().first() {
                    let preview: String = ch
                        .content
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(200)
                        .collect();
                    println!("  Preview:  {}...", preview);
                }

                // Show resource list
                for res in book.resources().iter().take(5) {
                    println!("  Resource: {} ({})", res.href, res.media_type);
                }
                if book.resources().len() > 5 {
                    println!("  ... and {} more resources", book.resources().len() - 5);
                }
            }
            Err(e) => {
                let elapsed = start.elapsed();
                fail += 1;
                println!("  FAIL ({:.1}ms): {}", elapsed.as_secs_f64() * 1000.0, e);
            }
        }
    }

    println!("\n{}", "=".repeat(80));
    println!(
        "Results: {} success, {} failed (out of {} ITOLITLS files)",
        success,
        fail,
        success + fail
    );
    println!("{}", "=".repeat(80));
}
