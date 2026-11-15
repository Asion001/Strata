//! `cargo bench -p strata-storage`: chunking + hashing throughput.
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use strata_storage::chunker;

fn data(n: usize) -> Vec<u8> {
    let mut x: u64 = 7;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            (x >> 33) as u8
        })
        .collect()
}

fn bench(c: &mut Criterion) {
    let d = data(64 * 1024 * 1024);
    let mut g = c.benchmark_group("ingest-64MiB");
    g.throughput(Throughput::Bytes(d.len() as u64));
    g.sample_size(10);
    g.bench_function("fastcdc", |b| b.iter(|| chunker::chunk_lengths(&d)));
    g.bench_function("blake3", |b| b.iter(|| blake3::hash(&d)));
    g.bench_function("fastcdc+blake3", |b| {
        b.iter(|| {
            let mut off = 0;
            for l in chunker::chunk_lengths(&d) {
                blake3::hash(&d[off..off + l]);
                off += l;
            }
        })
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
