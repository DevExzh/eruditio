//! Test a single LIT file. Usage: cargo run --bin test_one_lit -- <path>

use eruditio::domain::FormatReader;
use eruditio::formats::lit::LitReader;
use std::fs;
use std::io::Cursor;

fn main() {
    let path = std::env::args().nth(1).expect("Usage: test_one_lit <path>");
    let data = fs::read(&path).expect("read file");
    eprintln!("File: {} ({} bytes)", path, data.len());

    let reader = LitReader::new();
    let mut cursor = Cursor::new(data);
    match reader.read_book(&mut cursor) {
        Ok(book) => {
            println!("SUCCESS");
            println!("  Title:     {:?}", book.metadata.title);
            println!("  Authors:   {:?}", book.metadata.authors);
            println!("  Chapters:  {}", book.chapters().len());
            println!("  Resources: {}", book.resources().len());
            for (i, ch) in book.chapters().iter().enumerate().take(3) {
                let preview: String = ch
                    .content
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(200)
                    .collect();
                println!("  Ch{i}: {preview}...");
            }
        },
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        },
    }
}
