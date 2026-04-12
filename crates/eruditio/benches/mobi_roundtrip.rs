use criterion::{Criterion, black_box, criterion_group, criterion_main};
use eruditio::domain::FormatWriter;
use eruditio::domain::{Book, Chapter};
use eruditio::formats::mobi::MobiWriter;
use std::io::Cursor;

fn make_book(num_chapters: usize, chapter_size: usize) -> Book {
    let mut book = Book::new();
    book.metadata.title = Some("Benchmark Book".into());
    book.metadata.authors = vec!["Test Author".into()];
    let paragraph = "<p>Lorem ipsum dolor sit amet, consectetur adipiscing elit. \
        Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.</p>\n";
    for i in 0..num_chapters {
        let mut content = String::with_capacity(chapter_size);
        while content.len() < chapter_size {
            content.push_str(paragraph);
        }
        content.truncate(chapter_size);
        book.add_chapter(Chapter {
            title: Some(format!("Chapter {}", i + 1)),
            content,
            id: Some(format!("ch{}", i)),
        });
    }
    book
}

fn bench_mobi_write(c: &mut Criterion) {
    let book = make_book(50, 4096);
    let writer = MobiWriter::new();
    c.bench_function("mobi_write/50ch_4k", |b| {
        b.iter(|| {
            let mut buf = Cursor::new(Vec::with_capacity(256 * 1024));
            writer.write_book(black_box(&book), &mut buf).unwrap();
        })
    });

    let small_book = make_book(10, 1024);
    c.bench_function("mobi_write/10ch_1k", |b| {
        b.iter(|| {
            let mut buf = Cursor::new(Vec::with_capacity(64 * 1024));
            writer.write_book(black_box(&small_book), &mut buf).unwrap();
        })
    });
}

criterion_group!(benches, bench_mobi_write);
criterion_main!(benches);
