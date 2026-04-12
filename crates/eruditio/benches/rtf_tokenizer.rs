use criterion::{Criterion, black_box, criterion_group, criterion_main};
use eruditio::formats::rtf::tokenizer;

fn make_rtf(size: usize) -> Vec<u8> {
    let mut rtf = Vec::with_capacity(size);
    rtf.extend_from_slice(b"{\\rtf1\\ansi ");
    let text_chunk = b"This is some plain text content that will be repeated many times. ";
    let ctrl_chunk = b"\\par\\b Bold text here\\b0 ";
    let mut i = 0;
    while rtf.len() < size - 10 {
        if i % 5 == 0 {
            rtf.extend_from_slice(ctrl_chunk);
        } else {
            rtf.extend_from_slice(text_chunk);
        }
        i += 1;
    }
    rtf.push(b'}');
    rtf
}

fn bench_tokenize(c: &mut Criterion) {
    let rtf_100k = make_rtf(100_000);
    c.bench_function("rtf_tokenize/100k", |b| {
        b.iter(|| tokenizer::tokenize(black_box(&rtf_100k)))
    });

    let rtf_10k = make_rtf(10_000);
    c.bench_function("rtf_tokenize/10k", |b| {
        b.iter(|| tokenizer::tokenize(black_box(&rtf_10k)))
    });
}

criterion_group!(benches, bench_tokenize);
criterion_main!(benches);
