use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_slab_creation(c: &mut Criterion) {
    c.bench_function("slab_new", |b| {
        b.iter(|| exo_allocator::SlabAllocator::new(black_box(64), black_box(1024)));
    });
}

fn bench_bump_creation(c: &mut Criterion) {
    c.bench_function("bump_new", |b| {
        b.iter(|| exo_allocator::BumpAllocator::with_capacity(black_box(4096)));
    });
}

criterion_group!(benches, bench_slab_creation, bench_bump_creation);
criterion_main!(benches);
