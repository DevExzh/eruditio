//! Test harness for reading CHM files from assets/chm/.
//! Usage: cargo run --release --bin test_chm_files

use eruditio::domain::FormatReader;
use eruditio::formats::chm::ChmReader;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

fn main() {
    let dir = Path::new("assets/chm");
    if !dir.exists() {
        eprintln!("assets/chm/ directory not found");
        std::process::exit(1);
    }

    let mut entries: Vec<_> = fs::read_dir(dir)
        .expect("read dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "chm"))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let reader = ChmReader::new();
    let mut success = 0u32;
    let mut fail = 0u32;

    println!("{}", "=".repeat(80));
    println!("CHM File Test Report (Rust eruditio)");
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
            },
        };

        // Check ITSF magic
        let magic = if data.len() >= 4 {
            String::from_utf8_lossy(&data[0..4]).to_string()
        } else {
            format!("<{} bytes>", data.len())
        };

        println!("\n--- {} ({} bytes, magic: {:?}) ---", name, size, magic);

        let start = Instant::now();
        let mut cursor = Cursor::new(data);
        match reader.read_book(&mut cursor) {
            Ok(book) => {
                let elapsed = start.elapsed();
                success += 1;
                println!("  OK ({:.1}ms)", elapsed.as_secs_f64() * 1000.0);
                println!(
                    "  Title:     {}",
                    book.metadata.title.as_deref().unwrap_or("<none>")
                );
                println!("  Chapters:  {}", book.chapters().len());
                println!("  Resources: {}", book.resources().len());

                let total_text: usize = book.chapters().iter().map(|c| c.content.len()).sum();
                println!("  Total HTML: {} chars", total_text);

                // Preview first chapter
                if let Some(ch) = book.chapters().first() {
                    let preview: String = ch
                        .content
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(200)
                        .collect();
                    println!("  Preview:   {}...", preview.trim());
                }
            },
            Err(e) => {
                let elapsed = start.elapsed();
                fail += 1;
                println!("  FAIL ({:.1}ms): {}", elapsed.as_secs_f64() * 1000.0, e);
            },
        }
    }

    println!("\n{}", "=".repeat(80));
    println!(
        "Results: {} success, {} failed (out of {} files)",
        success,
        fail,
        entries.len()
    );
    println!("{}", "=".repeat(80));
}
