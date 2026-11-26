//! `cargo bench -p strata-server --bench range`: which chunks serve a byte
//! range (`range::covering`), for a 4 GiB file in 2,048 chunks of 2 MiB.
//! Pure computation on a small slice that stays in cache (warm); inputs and
//! results go through `black_box`.
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use strata_server::range;

fn covering(c: &mut Criterion) {
    let sizes: Vec<u64> = vec![2 << 20; 2048];
    let total: u64 = sizes.iter().sum();
    let mut g = c.benchmark_group("range");
    for (name, start, end) in [
        // A media player's first request, a seek to the middle, the tail.
        ("covering/head-1MiB", 0, 1 << 20),
        ("covering/middle-8MiB", total / 2, total / 2 + (8 << 20)),
        ("covering/last-byte", total - 1, total),
    ] {
        g.bench_function(name, |b| {
            b.iter(|| range::covering(black_box(&sizes), black_box(start), black_box(end)))
        });
    }
    g.bench_function("parse", |b| {
        b.iter(|| range::parse(black_box(Some("bytes=1048576-9437183")), black_box(total)))
    });
    g.finish();
}

criterion_group!(benches, covering);
criterion_main!(benches);
