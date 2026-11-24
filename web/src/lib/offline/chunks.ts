// Splits a file into the server's chunks (FastCDC) and hashes each one
// (BLAKE3), reading it 16 MB at a time.
import { createBLAKE3 } from 'hash-wasm';
import { MAX_SIZE, SINGLE_BLOB_MAX, cut } from '../chunker';
import type { Chunk } from './queue';

const READ = 16 * 1024 * 1024;

export async function chunkFile(file: Blob, signal?: AbortSignal, onProgress: (done: number) => void = () => {}): Promise<Chunk[]> {
  const hasher = await createBLAKE3();
  const hash = (d: Uint8Array) => {
    hasher.init();
    hasher.update(d);
    return hasher.digest('hex');
  };
  if (file.size <= SINGLE_BLOB_MAX) {
    const d = new Uint8Array(await file.arrayBuffer());
    onProgress(file.size);
    return [{ offset: 0, size: d.length, hash: hash(d) }];
  }
  const out: Chunk[] = [];
  let buf = new Uint8Array(0);
  let bufStart = 0;
  let readPos = 0;
  for (;;) {
    // (Cancelled: stop reading.)
    if (signal?.aborted) throw new DOMException('cancelled', 'AbortError');
    while (buf.length < MAX_SIZE && readPos < file.size) {
      const next = new Uint8Array(await file.slice(readPos, readPos + READ).arrayBuffer());
      readPos += next.length;
      const merged = new Uint8Array(buf.length + next.length);
      merged.set(buf);
      merged.set(next, buf.length);
      buf = merged;
    }
    if (buf.length === 0) break;
    const c = cut(buf);
    out.push({ offset: bufStart, size: c, hash: hash(buf.subarray(0, c)) });
    buf = buf.subarray(c);
    bufStart += c;
    onProgress(bufStart);
  }
  return out;
}

