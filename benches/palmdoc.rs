use criterion::{Criterion, black_box, criterion_group, criterion_main};
use eruditio::formats::common::compression::palmdoc;

fn make_text_record() -> Vec<u8> {
    let phrase = b"The quick brown fox jumps over the lazy dog. ";
    let mut data = Vec::with_capacity(4096);
    while data.len() + phrase.len() <= 4096 {
        data.extend_from_slice(phrase);
    }
    while data.len() < 4096 {
        data.push(b'X');
    }
    data
}

fn bench_compress(c: &mut Criterion) {
    let record = make_text_record();
    c.bench_function("palmdoc/compress_4k", |b| {
        b.iter(|| palmdoc::compress(black_box(&record)))
    });
}

fn bench_compress_reuse(c: &mut Criterion) {
    let record = make_text_record();
    let mut compressor = palmdoc::PalmDocCompressor::new();
    c.bench_function("palmdoc/compress_4k_reuse", |b| {
        b.iter(|| compressor.compress_record(black_box(&record)))
    });
}

fn bench_decompress(c: &mut Criterion) {
    let record = make_text_record();
    let compressed = palmdoc::compress(&record);
    c.bench_function("palmdoc/decompress_4k", |b| {
        b.iter(|| palmdoc::decompress(black_box(&compressed)).unwrap())
    });
}

fn bench_decompress_into(c: &mut Criterion) {
    let record = make_text_record();
    let compressed = palmdoc::compress(&record);
    let mut output = Vec::with_capacity(4096);
    c.bench_function("palmdoc/decompress_4k_into", |b| {
        b.iter(|| {
            output.clear();
            palmdoc::decompress_into(black_box(&compressed), &mut output).unwrap();
        })
    });
}

fn bench_compress_random(c: &mut Criterion) {
    // Low-entropy random-ish data (less compressible).
    let mut data = vec![0u8; 4096];
    for (i, b) in data.iter_mut().enumerate() {
        *b = ((i * 7 + 13) % 256) as u8;
    }
    c.bench_function("palmdoc/compress_4k_random", |b| {
        b.iter(|| palmdoc::compress(black_box(&data)))
    });
}

fn bench_compress_random_reuse(c: &mut Criterion) {
    let mut data = vec![0u8; 4096];
    for (i, b) in data.iter_mut().enumerate() {
        *b = ((i * 7 + 13) % 256) as u8;
    }
    let mut compressor = palmdoc::PalmDocCompressor::new();
    c.bench_function("palmdoc/compress_4k_random_reuse", |b| {
        b.iter(|| compressor.compress_record(black_box(&data)))
    });
}

fn bench_compress_multi_record(c: &mut Criterion) {
    // Simulate compressing a 200 KB book (50 records).
    let phrase = b"The quick brown fox jumps over the lazy dog. ";
    let mut book = Vec::with_capacity(200 * 1024);
    while book.len() + phrase.len() <= 200 * 1024 {
        book.extend_from_slice(phrase);
    }
    while book.len() < 200 * 1024 {
        book.push(b'.');
    }

    c.bench_function("palmdoc/compress_200k_book_reuse", |b| {
        b.iter(|| {
            let mut compressor = palmdoc::PalmDocCompressor::new();
            let mut records = Vec::new();
            for chunk in black_box(&book).chunks(4096) {
                records.push(compressor.compress_record(chunk));
            }
            records
        })
    });

    c.bench_function("palmdoc/compress_200k_book_into", |b| {
        b.iter(|| {
            let mut compressor = palmdoc::PalmDocCompressor::new();
            let mut records = Vec::new();
            let mut buf = Vec::with_capacity(4096);
            for chunk in black_box(&book).chunks(4096) {
                buf.clear();
                compressor.compress_record_into(chunk, &mut buf);
                records.push(buf.clone());
            }
            records
        })
    });
}

criterion_group!(
    benches,
    bench_compress,
    bench_compress_reuse,
    bench_decompress,
    bench_decompress_into,
    bench_compress_random,
    bench_compress_random_reuse,
    bench_compress_multi_record,
);
criterion_main!(benches);
