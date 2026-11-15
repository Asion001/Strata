import { describe, expect, it } from 'vitest';
import { GEAR, chunkLengths } from './chunker';

// Same generator as `ts_parity_vector` in crates/storage/src/chunker.rs.
function data(n: number): Uint8Array {
  const out = new Uint8Array(n);
  let x = 2463534242;
  for (let i = 0; i < n; i++) {
    x ^= x << 13; x >>>= 0;
    x ^= x >>> 17;
    x ^= x << 5; x >>>= 0;
    out[i] = x & 0xff;
  }
  return out;
}

describe('chunker', () => {
  it('matches the Rust gear table', () => {
    expect(Array.from(GEAR.slice(0, 4), (g) => g.toString(16).padStart(8, '0'))).toEqual(RUST_GEAR);
  });
  it('matches Rust chunk boundaries', () => {
    expect(chunkLengths(data(24 * 1024 * 1024 + 99))).toEqual(RUST_LENGTHS);
  });
  it('keeps small files whole', () => {
    expect(chunkLengths(new Uint8Array(100))).toEqual([100]);
  });
});

// Filled in from `cargo test -p strata-storage ts_parity_vector -- --nocapture`.
const RUST_GEAR: string[] = ["fa9fc72f", "4d3ee452", "a7297d88", "9b9f0144"];
const RUST_LENGTHS: number[] = [2281310, 2884615, 772734, 553025, 2277525, 1688973, 2097379, 2229111, 597123, 2860054, 1057328, 2827175, 2223653, 815918];
