//! `cargo bench -p strata-storage --bench micro`: hashing and chunking.
//!
//! Cache state, per bench:
//! * `blake3/1MiB` is warm: the input fits in L2/L3 and every iteration
//!   hashes the same, already-cached bytes.
//! * `*/64MiB` is cold: 64 MiB is larger than the CPU caches, so every
//!   iteration streams its input from main memory again.
//!
//! Inputs and results go through `black_box`, so no work is optimized away.
use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use strata_storage::chunker;

const MIB: usize = 1024 * 1024;

fn data(n: usize) -> Vec<u8> {
    let mut x: u64 = 7;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            (x >> 33) as u8
        })
        .collect()
}

fn hashing(c: &mut Criterion) {
    let mut g = c.benchmark_group("blake3");
    for (name, n) in [("1MiB", MIB), ("64MiB", 64 * MIB)] {
        let d = data(n);
        g.throughput(Throughput::Bytes(n as u64));
        if n > MIB {
            g.sample_size(20);
        }
        // Single-threaded, as the server hashes (one chunk per call).
        g.bench_function(name, |b| b.iter(|| blake3::hash(black_box(&d))));
    }
    g.finish();
}

fn chunking(c: &mut Criterion) {
    let d = data(64 * MIB);
    let mut g = c.benchmark_group("fastcdc");
    g.throughput(Throughput::Bytes(d.len() as u64));
    g.sample_size(20);
    g.bench_function("64MiB", |b| {
        b.iter(|| chunker::chunk_lengths(black_box(&d)))
    });
    // What an upload costs: cut, then hash each chunk.
    g.bench_function("64MiB+blake3", |b| {
        b.iter(|| {
            let d = black_box(&d);
            let mut off = 0;
            for l in chunker::chunk_lengths(d) {
                black_box(blake3::hash(&d[off..off + l]));
                off += l;
            }
        })
    });
    g.finish();
}

criterion_group!(benches, hashing, chunking);
criterion_main!(benches);
