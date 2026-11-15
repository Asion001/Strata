// Resumable chunked upload: chunk with FastCDC, hash with BLAKE3 (WASM),
// ask the server what it is missing, send only that, then commit.
import { createBLAKE3 } from 'hash-wasm';
import { api, ApiError } from './api';
import { MAX_SIZE, SINGLE_BLOB_MAX, cut } from './chunker';

export interface UploadTask {
  id: number;
  file: File;
  path: string;
  status: 'queued' | 'hashing' | 'uploading' | 'done' | 'error';
  progress: number; // 0..1
  sent: number; // bytes sent over the network
  skipped: number; // bytes the server already had
  error?: string;
}

interface Chunk {
  offset: number;
  size: number;
  hash: string;
}

const READ = 16 * 1024 * 1024;

async function chunkFile(file: File, onProgress: (done: number) => void): Promise<Chunk[]> {
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

function resumeKey(t: UploadTask) {
  return `strata-upload:${t.path}:${t.file.size}:${t.file.lastModified}`;
}

export async function runUpload(t: UploadTask, update: () => void): Promise<void> {
  try {
    t.status = 'hashing';
    update();
    const chunks = await chunkFile(t.file, (done) => {
      t.progress = (done / Math.max(1, t.file.size)) * 0.15;
      update();
    });

    let id: string | null = null;
    let missing: number[] = [];
    const key = resumeKey(t);
    try {
      const saved = localStorage.getItem(key);
      if (saved) {
        const s = await api('GET', `/api/uploads/${saved}`);
        id = saved;
        missing = s.missing;
      }
    } catch {
      id = null;
    }
    if (!id) {
      const r = await api('POST', '/api/uploads', {
        path: t.path,
        size: t.file.size,
        mtime: new Date(t.file.lastModified).toISOString(),
        chunks: chunks.map((c) => ({ hash: c.hash, size: c.size }))
      });
      id = r.id as string;
      missing = r.missing;
      try {
        localStorage.setItem(key, id);
      } catch {}
    }

    // Dedupe identical chunks within the file.
    const seen = new Set<string>();
    const todo = missing.map((i) => chunks[i]).filter((c) => !seen.has(c.hash) && seen.add(c.hash));
    const todoBytes = todo.reduce((a, c) => a + c.size, 0);
    t.skipped = t.file.size - todoBytes;
    t.status = 'uploading';
    update();

    let sent = 0;
    let next = 0;
    const worker = async () => {
      while (next < todo.length) {
        const c = todo[next++];
        for (let attempt = 0; ; attempt++) {
          try {
            const r = await fetch(`/api/uploads/${id}/chunks/${c.hash}`, {
              method: 'PUT',
              headers: { 'x-strata': '1' },
              body: t.file.slice(c.offset, c.offset + c.size)
            });
            if (!r.ok) throw new ApiError(r.status, (await r.json().catch(() => ({}))).error ?? r.statusText);
            break;
          } catch (e) {
            if (attempt >= 4 || (e instanceof ApiError && e.status < 500)) throw e;
            await new Promise((res) => setTimeout(res, 1000 * 2 ** attempt));
          }
        }
        sent += c.size;
        t.sent = sent;
        t.progress = 0.15 + 0.8 * (todoBytes ? sent / todoBytes : 1);
        update();
      }
    };
    await Promise.all([worker(), worker(), worker()]);
    await api('POST', `/api/uploads/${id}/commit`, {});
    try {
      localStorage.removeItem(key);
    } catch {}
    t.status = 'done';
    t.progress = 1;
  } catch (e) {
    t.status = 'error';
    t.error = e instanceof Error ? e.message : String(e);
  }
  update();
}
