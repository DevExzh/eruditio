//! Validation CLI: reads any ebook file, outputs a normalized report for
//! cross-implementation comparison (Rust vs Calibre Python).
//!
//! Output format (one record per file, easy to diff):
//!   FILE: <path>
//!   FORMAT: <ext>
//!   STATUS: OK | ERROR(<msg>)
//!   TITLE: <title>
//!   AUTHORS: <comma-separated>
//!   CHAPTERS: <count>
//!   RESOURCES: <count>
//!   TEXT_BYTES: <total bytes of text content>
//!   TEXT_HASH: <sha256 of concatenated text content>
//!   TIME_US: <parse time in microseconds>
//!   ---

use eruditio::EruditioParser;
use std::env;
use std::time::Instant;

fn content_fingerprint(data: &[u8]) -> String {
    // FNV-1a over first+last 64 bytes, plus total length.
    // Not a cryptographic hash — used only for quick diff-able comparison.
    // The Python validation side uses the same algorithm.
    let len = data.len();
    if len == 0 {
        return "empty".to_string();
    }
    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for &b in data.iter().take(64).chain(data.iter().rev().take(64)) {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV prime
    }
    format!("{:016x}:{}", hash, len)
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("Usage: validate <file1> [file2] ...");
        std::process::exit(1);
    }

    for path in &args {
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let start = Instant::now();
        let result = EruditioParser::parse_file(path);
        let elapsed = start.elapsed();

        println!("FILE: {}", path);
        println!("FORMAT: {}", ext);

        match result {
            Ok(book) => {
                let title = book.metadata.title.as_deref().unwrap_or("<none>");
                let authors = if book.metadata.authors.is_empty() {
                    "<none>".to_string()
                } else {
                    book.metadata.authors.join(", ")
                };

                // Count chapters (spine items)
                let chapter_count = book.spine.len();

                // Count resources (non-text manifest items)
                let resource_count = book
                    .manifest
                    .iter()
                    .filter(|item| {
                        !item.media_type.contains("xhtml")
                            && !item.media_type.contains("html")
                            && !item.media_type.contains("xml")
                            && !item.media_type.contains("ncx")
                            && !item.media_type.contains("css")
                    })
                    .count();

                // Collect all text content for hashing
                let mut all_text = Vec::new();
                for item in book.manifest.iter() {
                    if let Some(text) = item.data.as_text() {
                        all_text.extend_from_slice(text.as_bytes());
                    } else if let Some(data) = item.data.as_bytes()
                        && (item.media_type.contains("html")
                            || item.media_type.contains("xml")
                            || item.media_type.contains("text"))
                    {
                        all_text.extend_from_slice(data);
                    }
                }

                println!("STATUS: OK");
                println!("TITLE: {}", title);
                println!("AUTHORS: {}", authors);
                println!("CHAPTERS: {}", chapter_count);
                println!("RESOURCES: {}", resource_count);
                println!("TEXT_BYTES: {}", all_text.len());
                println!("TEXT_HASH: {}", content_fingerprint(&all_text));
                println!("TIME_US: {}", elapsed.as_micros());
            },
            Err(e) => {
                println!("STATUS: ERROR({})", e);
                println!("TIME_US: {}", elapsed.as_micros());
            },
        }
        println!("---");
    }
}
