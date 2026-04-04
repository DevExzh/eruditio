//! Standalone binary for memory profiling with DHAT or Valgrind.
//!
//! Usage:
//!   # DHAT profiling (generates dhat-heap.json):
//!   cargo run --features dhat-heap --release --bin memory_profile -- <file>
//!
//!   # Valgrind Massif:
//!   cargo build --release --bin memory_profile
//!   valgrind --tool=massif ./target/release/memory_profile <file>

#[cfg(feature = "dhat-heap")]
extern crate eruditio; // force linkage so the library's #[global_allocator] is active

fn main() {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: memory_profile <ebook-file> [--repeat N]");
        std::process::exit(1);
    }

    let path = &args[1];
    let repeat = if args.len() >= 4 && args[2] == "--repeat" {
        args[3].parse::<usize>().unwrap_or(1)
    } else {
        1
    };

    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    eprintln!("Profiling: {} (format: {}, repeat: {})", path, ext, repeat);

    for i in 0..repeat {
        let data = std::fs::read(path).expect("Failed to read file");
        let input_size = data.len();
        let mut cursor = std::io::Cursor::new(data);

        match eruditio::EruditioParser::parse(&mut cursor, Some(ext)) {
            Ok(book) => {
                eprintln!(
                    "  [run {}] OK — {} chapters, {} manifest items, input {} bytes",
                    i + 1,
                    book.chapter_count(),
                    book.manifest.len(),
                    input_size,
                );
            },
            Err(e) => {
                eprintln!("  [run {}] ERROR: {}", i + 1, e);
            },
        }
    }

    eprintln!("Done. DHAT profiler will dump stats on exit (if enabled).");
}
