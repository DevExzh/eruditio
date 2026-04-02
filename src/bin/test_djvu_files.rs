//! Test harness for reading DJVU files from assets/djvu/.
//! Usage: cargo run --release --bin test_djvu_files

use eruditio::domain::FormatReader;
use eruditio::formats::djvu::DjvuReader;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

fn main() {
    let dir = Path::new("assets/djvu");
    if !dir.exists() {
        eprintln!("assets/djvu/ directory not found");
        std::process::exit(1);
    }

    let mut entries: Vec<_> = fs::read_dir(dir)
        .expect("read dir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext == "djvu" || ext == "djv")
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let reader = DjvuReader::new();
    let mut success = 0u32;
    let mut fail = 0u32;
    let mut no_text = 0u32;

    println!("{}", "=".repeat(80));
    println!("DJVU File Test Report (Rust eruditio)");
    println!("{}", "=".repeat(80));

    for entry in &entries {
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy();
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

        let data = match fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                println!("\n--- {} ({} bytes) ---", name, size);
                println!("  ERROR: Could not read file: {}", e);
                fail += 1;
                continue;
            }
        };

        println!("\n--- {} ({} bytes) ---", name, size);

        let start = Instant::now();
        let mut cursor = Cursor::new(data);
        match reader.read_book(&mut cursor) {
            Ok(book) => {
                let elapsed = start.elapsed();
                let chapters = book.chapters();
                let total_text: usize = chapters.iter().map(|c| c.content.len()).sum();
                if total_text == 0 || chapters.is_empty() {
                    no_text += 1;
                    println!("  NO_TEXT ({:.1}ms) - no text layer found", elapsed.as_secs_f64() * 1000.0);
                } else {
                    success += 1;
                    println!("  OK ({:.1}ms)", elapsed.as_secs_f64() * 1000.0);
                    println!("  Pages with text: {}", chapters.len());
                    println!("  Total text: {} chars", total_text);
                    // Preview first page
                    if let Some(ch) = chapters.first() {
                        let preview: String = ch.content
                            .chars()
                            .filter(|c| !c.is_control() && *c != '<' && *c != '>')
                            .take(200)
                            .collect();
                        println!("  Preview: {}...", preview.trim());
                    }
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
        "Results: {} with text, {} no text, {} failed (out of {} files)",
        success, no_text, fail, entries.len()
    );
    println!("{}", "=".repeat(80));
}
