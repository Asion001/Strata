// Unsaved editor drafts in IndexedDB, so a crash or closed tab loses nothing.

export interface Draft {
  key: string;
  /** ETag of the version the draft was based on. */
  baseEtag: string | null;
  text: string;
  savedAt: number;
}

const DB = 'strata-editor';
const STORE = 'drafts';

export function draftKey(userId: number, mountId: number | null, path: string): string {
  return `${userId}|${mountId ?? ''}|${path}`;
}

function open(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB, 1);
    req.onupgradeneeded = () => req.result.createObjectStore(STORE, { keyPath: 'key' });
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

async function tx<T>(mode: IDBTransactionMode, fn: (s: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const db = await open();
  try {
    return await new Promise<T>((resolve, reject) => {
      const t = db.transaction(STORE, mode);
      const req = fn(t.objectStore(STORE));
      t.oncomplete = () => resolve(req.result);
      t.onerror = () => reject(t.error);
      t.onabort = () => reject(t.error);
    });
  } finally {
    db.close();
  }
}

export const saveDraft = (d: Draft) => tx('readwrite', (s) => s.put(d)).then(() => undefined);
export const getDraft = (key: string) =>
  tx<Draft | undefined>('readonly', (s) => s.get(key) as IDBRequest<Draft | undefined>);
export const clearDraft = (key: string) => tx('readwrite', (s) => s.delete(key)).then(() => undefined);

/** Drafts are kept for 30 days at most. */
export const DRAFT_TTL_MS = 30 * 24 * 3600 * 1000;

/** Remove drafts older than the TTL. */
export async function expireDrafts(now = Date.now()): Promise<void> {
  const all = await tx<Draft[]>('readonly', (s) => s.getAll() as IDBRequest<Draft[]>);
  for (const d of all) if (now - d.savedAt > DRAFT_TTL_MS) await clearDraft(d.key);
}

/** Forget every draft in this browser (on sign-out: drafts hold file contents). */
export function clearAllDrafts(): Promise<void> {
  return new Promise((resolve) => {
    const req = indexedDB.deleteDatabase(DB);
    req.onsuccess = req.onerror = req.onblocked = () => resolve();
  });
}
