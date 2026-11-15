//! FastCDC content-defined chunking (normalised chunking, Xia et al. 2016).
//!
//! The gear table is 32-bit and generated from a fixed xorshift32 seed so the
//! web client (`web/src/lib/chunker.ts`) produces byte-identical boundaries.
//! That lets the browser hash chunks locally and skip uploading chunks the
//! server already stores.

/// Files up to this size are stored as a single blob.
pub const SINGLE_BLOB_MAX: usize = 4 * 1024 * 1024;
pub const MIN_SIZE: usize = 512 * 1024;
pub const AVG_SIZE: usize = 2 * 1024 * 1024;
pub const MAX_SIZE: usize = 8 * 1024 * 1024;

const AVG_BITS: u32 = 21; // log2(AVG_SIZE)
// Normalised chunking: stricter mask before the average size, looser after.
const MASK_S: u32 = mask(AVG_BITS + 2);
const MASK_L: u32 = mask(AVG_BITS - 2);

const fn mask(bits: u32) -> u32 {
    // Use the high bits: they depend on the last 32 bytes rolled through.
    (((1u64 << bits) - 1) as u32) << (32 - bits)
}

const fn gear() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut x: u32 = 0x5354_5241; // "STRA"
    let mut i = 0;
    while i < 256 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        t[i] = x;
        i += 1;
    }
    t
}

pub static GEAR: [u32; 256] = gear();

/// Length of the first chunk in `data`. `data` must hold at least `MAX_SIZE`
/// bytes unless it is the tail of the input.
pub fn cut(data: &[u8]) -> usize {
    let mut n = data.len();
    if n <= MIN_SIZE {
        return n;
    }
    if n > MAX_SIZE {
        n = MAX_SIZE;
    }
    let normal = AVG_SIZE.min(n);
    let mut h: u32 = 0;
    let mut i = MIN_SIZE;
    while i < normal {
        h = (h << 1).wrapping_add(GEAR[data[i] as usize]);
        if h & MASK_S == 0 {
            return i + 1;
        }
        i += 1;
    }
    while i < n {
        h = (h << 1).wrapping_add(GEAR[data[i] as usize]);
        if h & MASK_L == 0 {
            return i + 1;
        }
        i += 1;
    }
    n
}

/// Chunk boundaries for a complete in-memory buffer. Small inputs
/// (`<= SINGLE_BLOB_MAX`) are one chunk.
pub fn chunk_lengths(data: &[u8]) -> Vec<usize> {
    if data.len() <= SINGLE_BLOB_MAX {
        return vec![data.len()];
    }
    let mut out = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let c = cut(rest);
        out.push(c);
        rest = &rest[c..];
    }
    out
}

/// Streaming chunker: feed bytes, get complete chunks out.
pub struct StreamChunker {
    buf: Vec<u8>,
    total: u64,
    emitted_any: bool,
}

impl Default for StreamChunker {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamChunker {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(MAX_SIZE * 2),
            total: 0,
            emitted_any: false,
        }
    }

    /// Push bytes; returns any chunks that are now final.
    pub fn push(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.total += data.len() as u64;
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        // Small files are a single chunk, so we cannot cut until we know the
        // input is bigger than SINGLE_BLOB_MAX.
        if self.total as usize <= SINGLE_BLOB_MAX && !self.emitted_any {
            return out;
        }
        while self.buf.len() >= MAX_SIZE {
            let c = cut(&self.buf);
            out.push(self.buf.drain(..c).collect());
            self.emitted_any = true;
        }
        out
    }

    /// Flush the remaining bytes as final chunks.
    pub fn finish(mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        if !self.emitted_any && self.buf.len() <= SINGLE_BLOB_MAX {
            out.push(std::mem::take(&mut self.buf));
            return out;
        }
        let mut rest: &[u8] = &self.buf;
        while !rest.is_empty() {
            let c = cut(rest);
            out.push(rest[..c].to_vec());
            rest = &rest[c..];
        }
        out
    }
}

#[cfg(test)]
const PARITY_GEAR: [&str; 4] = ["fa9fc72f", "4d3ee452", "a7297d88", "9b9f0144"];
#[cfg(test)]
const PARITY_LENGTHS: &[usize] = &[
    2281310, 2884615, 772734, 553025, 2277525, 1688973, 2097379, 2229111, 597123, 2860054, 1057328,
    2827175, 2223653, 815918,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn pseudo(n: usize) -> Vec<u8> {
        let mut x: u64 = 42;
        (0..n)
            .map(|_| {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (x >> 33) as u8
            })
            .collect()
    }

    #[test]
    fn small_is_single_chunk() {
        assert_eq!(chunk_lengths(&pseudo(1000)), vec![1000]);
        assert_eq!(chunk_lengths(&[]), vec![0]);
    }

    #[test]
    fn streaming_matches_batch() {
        let data = pseudo(40 * 1024 * 1024 + 123);
        let batch = chunk_lengths(&data);
        assert!(batch.len() > 5);
        assert!(batch.iter().all(|&c| c <= MAX_SIZE));
        let mut s = StreamChunker::new();
        let mut got = Vec::new();
        for piece in data.chunks(777_777) {
            got.extend(s.push(piece).into_iter().map(|c| c.len()));
        }
        got.extend(s.finish().into_iter().map(|c| c.len()));
        assert_eq!(got, batch);
    }

    #[test]
    fn insert_shifts_few_chunks() {
        let data = pseudo(32 * 1024 * 1024);
        let mut edited = data.clone();
        edited.splice(10_000_000..10_000_000, [1u8, 2, 3]);
        let hashes = |d: &[u8]| {
            let mut off = 0;
            chunk_lengths(d)
                .into_iter()
                .map(|l| {
                    let h = blake3::hash(&d[off..off + l]);
                    off += l;
                    h
                })
                .collect::<std::collections::HashSet<_>>()
        };
        let a = hashes(&data);
        let b = hashes(&edited);
        let shared = a.intersection(&b).count();
        assert!(shared + 3 >= a.len(), "shared {shared} of {}", a.len());
    }

    /// Data for the parity vector shared with `web/src/lib/chunker.test.ts`.
    pub(crate) fn xorshift_data(n: usize) -> Vec<u8> {
        let mut x: u32 = 2463534242;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x & 0xff) as u8
            })
            .collect()
    }

    #[test]
    fn ts_parity_vector() {
        let gear: Vec<String> = GEAR[..4].iter().map(|g| format!("{g:08x}")).collect();
        let lens = chunk_lengths(&xorshift_data(24 * 1024 * 1024 + 99));
        println!("RUST_GEAR = {gear:?}\nRUST_LENGTHS = {lens:?}");
        assert_eq!(gear, PARITY_GEAR);
        assert_eq!(lens, PARITY_LENGTHS);
    }
}
