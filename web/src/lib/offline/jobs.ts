// Upload jobs as the UI sees them: their shape and progress (no storage or
// network code: this part loads with the app; the queue itself, lazily).

export interface Chunk {
  offset: number;
  size: number;
  hash: string;
}

/** `decide`: its name is taken; it waits for the user (Replace, Keep both
 *  or Skip) without holding up the others. */
export type State = 'queued' | 'hashing' | 'uploading' | 'committing' | 'decide' | 'done' | 'failed' | 'cancelled';

export interface Job {
  id: string;
  user: number;
  /** Target path (folder and name). */
  path: string;
  blob: Blob;
  size: number;
  lastModified: number;
  state: State;
  /** The server's upload session, once created. */
  session: string | null;
  chunks: Chunk[] | null;
  /** Hashes of the chunks the server has confirmed. */
  sent: string[];
  /** Bytes the server already had (not sent). */
  skipped: number;
  error?: string;
  created: number;
  /** The user's answer to a taken name (none yet: ask). */
  conflict?: 'replace' | 'rename';
  /** In `decide`: what is there. */
  existing?: { is_dir: boolean; size?: number | null; mtime?: string | null };
}

/** Not finished (a job waiting for a decision too: it keeps its bytes). */
export const ACTIVE: State[] = ['queued', 'hashing', 'uploading', 'committing', 'decide'];
export const isActive = (j: Job) => ACTIVE.includes(j.state);

/** Bytes of the job's chunks the server has. */
export function doneBytes(j: Job): number {
  if (!j.chunks) return 0;
  const sent = new Set(j.sent);
  return j.chunks.reduce((n, c) => n + (sent.has(c.hash) ? c.size : 0), 0);
}

/** 0..1 for a progress bar. */
export function progress(j: Job): number {
  if (j.state === 'done') return 1;
  if (!j.chunks) return 0;
  return Math.min(1, doneBytes(j) / Math.max(1, j.size));
}
