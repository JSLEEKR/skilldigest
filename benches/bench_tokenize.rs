//! Criterion benchmark for the tokenizer hot path.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use skilldigest::tokenize;

const SAMPLE_SHORT: &str = "Use `Bash(ls)` to enumerate directory contents.";
const SAMPLE_MEDIUM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. \
Phasellus imperdiet massa eu massa posuere, at euismod ex tempus. \
Sed maximus tellus vitae consequat vehicula. \
Morbi dapibus nulla nec lorem dictum, sed gravida tortor volutpat.";

fn bench_cl100k_short(c: &mut Criterion) {
    let t = tokenize::by_name("cl100k").unwrap();
    c.bench_function("cl100k_short", |b| {
        b.iter(|| {
            let n = t.count(black_box(SAMPLE_SHORT));
            black_box(n);
        })
    });
}

fn bench_cl100k_medium(c: &mut Criterion) {
    let t = tokenize::by_name("cl100k").unwrap();
    c.bench_function("cl100k_medium", |b| {
        b.iter(|| {
            let n = t.count(black_box(SAMPLE_MEDIUM));
            black_box(n);
        })
    });
}

fn bench_llama3_approx(c: &mut Criterion) {
    let t = tokenize::by_name("llama3").unwrap();
    c.bench_function("llama3_medium", |b| {
        b.iter(|| {
            let n = t.count(black_box(SAMPLE_MEDIUM));
            black_box(n);
        })
    });
}

criterion_group!(
    benches,
    bench_cl100k_short,
    bench_cl100k_medium,
    bench_llama3_approx
);
criterion_main!(benches);
