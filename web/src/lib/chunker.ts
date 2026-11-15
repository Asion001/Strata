// FastCDC chunker, byte-for-byte identical to crates/storage/src/chunker.rs.
// The browser splits files the same way the server would, so it can ask the
// server which chunks it already has and skip uploading them.

export const SINGLE_BLOB_MAX = 4 * 1024 * 1024;
export const MIN_SIZE = 512 * 1024;
export const AVG_SIZE = 2 * 1024 * 1024;
export const MAX_SIZE = 8 * 1024 * 1024;

const mask = (bits: number) => ((2 ** bits - 1) * 2 ** (32 - bits)) >>> 0;
const MASK_S = mask(21 + 2);
const MASK_L = mask(21 - 2);

export const GEAR = (() => {
  const t = new Uint32Array(256);
  let x = 0x53545241;
  for (let i = 0; i < 256; i++) {
    x ^= x << 13;
    x >>>= 0;
    x ^= x >>> 17;
    x ^= x << 5;
    x >>>= 0;
    t[i] = x;
  }
  return t;
})();

/** Length of the first chunk of `data` (see the Rust `cut`). */
export function cut(data: Uint8Array): number {
  let n = data.length;
  if (n <= MIN_SIZE) return n;
  if (n > MAX_SIZE) n = MAX_SIZE;
  const normal = Math.min(AVG_SIZE, n);
  let h = 0;
  let i = MIN_SIZE;
  for (; i < normal; i++) {
    h = ((h << 1) + GEAR[data[i]]) >>> 0;
    if ((h & MASK_S) === 0) return i + 1;
  }
  for (; i < n; i++) {
    h = ((h << 1) + GEAR[data[i]]) >>> 0;
    if ((h & MASK_L) === 0) return i + 1;
  }
  return n;
}

export function chunkLengths(data: Uint8Array): number[] {
  if (data.length <= SINGLE_BLOB_MAX) return [data.length];
  const out: number[] = [];
  let off = 0;
  while (off < data.length) {
    const c = cut(data.subarray(off));
    out.push(c);
    off += c;
  }
  return out;
}
