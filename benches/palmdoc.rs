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

fn bench_decompress(c: &mut Criterion) {
    let record = make_text_record();
    let compressed = palmdoc::compress(&record);
    c.bench_function("palmdoc/decompress_4k", |b| {
        b.iter(|| palmdoc::decompress(black_box(&compressed)).unwrap())
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

criterion_group!(
    benches,
    bench_compress,
    bench_decompress,
    bench_compress_random
);
criterion_main!(benches);
