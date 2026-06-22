use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn benchmark_example(c: &mut Criterion) {
    c.bench_function("example", |b| {
        b.iter(|| {
            black_box(2 + 2)
        });
    });
}

criterion_group!(benches, benchmark_example);
criterion_main!(benches);
