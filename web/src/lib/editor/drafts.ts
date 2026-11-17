// Unsaved editor drafts in IndexedDB, so a crash or closed tab loses nothing.

export interface Draft {
  key: string;
  /** ETag of the version the draft was based on. */
  baseEtag: string | null;
  /** BLAKE3 (hex) of the bytes the draft was based on. */
  baseHash?: string | null;
  text: string;
  savedAt: number;
  /** The editor instance (tab) that wrote it: a tab only clears its own. */
  tab?: string;
}

const DB = 'strata-editor';
const STORE = 'drafts';

export function draftKey(userId: number, mountId: number | null, path: string): string {
  return `${userId}|${mountId ?? ''}|${path}`;
}

function open(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB, 2);
    req.onupgradeneeded = (e) => {
      const db = req.result;
      const store =
        e.oldVersion < 1 ? db.createObjectStore(STORE, { keyPath: 'key' }) : req.transaction!.objectStore(STORE);
      if (!store.indexNames.contains('savedAt')) store.createIndex('savedAt', 'savedAt');
    };
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

/** Delete the draft only if this tab wrote it (one transaction), so saving in
 *  one tab never throws away another tab's unsaved work on the same file. */
export async function clearOwnDraft(key: string, tab: string): Promise<void> {
  const db = await open();
  try {
    await new Promise<void>((resolve, reject) => {
      const t = db.transaction(STORE, 'readwrite');
      const s = t.objectStore(STORE);
      const req = s.get(key);
      req.onsuccess = () => {
        const d = req.result as Draft | undefined;
        if (d && d.tab === tab) s.delete(key);
      };
      t.oncomplete = () => resolve();
      t.onerror = () => reject(t.error);
    });
  } finally {
    db.close();
  }
}

/** Drafts are kept for 30 days at most. */
export const DRAFT_TTL_MS = 30 * 24 * 3600 * 1000;

/** Remove drafts older than the TTL (keys only, one transaction). */
export async function expireDrafts(now = Date.now()): Promise<void> {
  const db = await open();
  try {
    await new Promise<void>((resolve, reject) => {
      const t = db.transaction(STORE, 'readwrite');
      const req = t.objectStore(STORE).index('savedAt').openKeyCursor(IDBKeyRange.upperBound(now - DRAFT_TTL_MS, true));
      req.onsuccess = () => {
        const c = req.result;
        if (!c) return;
        t.objectStore(STORE).delete(c.primaryKey);
        c.continue();
      };
      t.oncomplete = () => resolve();
      t.onerror = () => reject(t.error);
    });
  } finally {
    db.close();
  }
}

// Who is signed in, shared by every tab (localStorage is synchronous and
// cross-tab). Autosave only writes while it names the draft's user.
const AUTH_KEY = 'strata-signed-in';

export function markSignedIn(userId: number) {
  try {
    localStorage.setItem(AUTH_KEY, String(userId));
  } catch {}
}

export function markSignedOut() {
  try {
    localStorage.removeItem(AUTH_KEY);
  } catch {}
}

export function isSignedIn(userId: number): boolean {
  try {
    return localStorage.getItem(AUTH_KEY) === String(userId);
  } catch {
    return true; // no storage at all: drafts can't outlive the tab anyway
  }
}

/** Forget every draft in this browser (on sign-out: drafts hold file
 *  contents). Waits for the deletion; gives up after a few seconds if an
 *  open connection somewhere keeps it blocked. */
export function clearAllDrafts(): Promise<void> {
  return new Promise((resolve) => {
    const req = indexedDB.deleteDatabase(DB);
    const timer = setTimeout(resolve, 3000);
    req.onsuccess = req.onerror = () => {
      clearTimeout(timer);
      resolve();
    };
  });
}

/** Drop drafts that belong to other users (when someone signs in here). */
export async function keepOnlyUser(userId: number): Promise<void> {
  const keys = await tx<IDBValidKey[]>('readonly', (s) => s.getAllKeys());
  for (const k of keys) if (!String(k).startsWith(`${userId}|`)) await clearDraft(String(k));
}
