//! Debug test for a single LIT file with tracing.
//! Usage: cargo run --bin debug_lit -- <path>

use eruditio::formats::lit::LitReader;
use eruditio::domain::FormatReader;
use std::fs;
use std::io::Cursor;
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).expect("Usage: debug_lit <path>");
    let data = fs::read(&path).expect("read file");
    eprintln!("[DEBUG] File: {} ({} bytes)", path, data.len());

    // Try to manually trace the LitContainer::parse steps
    // Since LitContainer is private, we'll instrument through read_book
    // and add timeouts between steps by watching wall time
    let reader = LitReader::new();
    let mut cursor = Cursor::new(data);

    let start = Instant::now();
    eprintln!("[DEBUG] Starting read_book...");

    // Set up a watchdog thread
    let watchdog = std::thread::spawn(move || {
        for i in 1..=30 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            eprintln!("[WATCHDOG] {}s elapsed", i);
        }
        eprintln!("[WATCHDOG] 30s timeout - aborting");
        std::process::exit(2);
    });

    match reader.read_book(&mut cursor) {
        Ok(book) => {
            let elapsed = start.elapsed();
            eprintln!("[DEBUG] read_book succeeded in {:.1}ms", elapsed.as_secs_f64() * 1000.0);
            println!("Title:     {:?}", book.metadata.title);
            println!("Authors:   {:?}", book.metadata.authors);
            println!("Chapters:  {}", book.chapters().len());
            println!("Resources: {}", book.resources().len());
            for (i, ch) in book.chapters().iter().enumerate().take(3) {
                let preview: String = ch.content.chars()
                    .filter(|c| !c.is_control())
                    .take(200)
                    .collect();
                println!("Ch{i}: {preview}...");
            }
        }
        Err(e) => {
            let elapsed = start.elapsed();
            eprintln!("[DEBUG] read_book failed in {:.1}ms: {e}", elapsed.as_secs_f64() * 1000.0);
            std::process::exit(1);
        }
    }

    drop(watchdog);
}
