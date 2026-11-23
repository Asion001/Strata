// The offline store, shared by the app and the service worker: the
// recent-files index and the upload queue (IndexedDB). Everything in it is
// keyed by user and is deleted on sign-out.

export const DB = 'strata-offline';
export type Store = 'recent' | 'jobs' | 'blobs';

let opening: Promise<IDBDatabase> | null = null;

export function db(): Promise<IDBDatabase> {
  if (opening) return opening;
  const current: Promise<IDBDatabase> = new Promise<IDBDatabase>((resolve, reject) => {
    const req = indexedDB.open(DB, 1);
    req.onupgradeneeded = () => {
      const d = req.result;
      d.createObjectStore('recent', { keyPath: 'key' }).createIndex('user', 'user');
      d.createObjectStore('jobs', { keyPath: 'id' }).createIndex('user', 'user');
      // A job's bytes, apart from its (often rewritten) state.
      d.createObjectStore('blobs');
    };
    req.onsuccess = () => {
      const d = req.result;
      // Another context deletes the database (sign-out): let it.
      d.onversionchange = () => {
        d.close();
        opening = null;
      };
      // The browser closed it (WebKit does, e.g. for a backgrounded app).
      d.onclose = () => {
        if (opening === current) opening = null;
      };
      resolve(d);
    };
    req.onerror = () => {
      opening = null;
      reject(req.error);
    };
  });
  opening = current;
  return current;
}

/** A transaction, reopening the database once if the old connection is gone. */
async function transaction(store: Store, mode: IDBTransactionMode): Promise<IDBTransaction> {
  try {
    return (await db()).transaction(store, mode);
  } catch (e) {
    if ((e as DOMException)?.name !== 'InvalidStateError') throw e;
    opening = null;
    return (await db()).transaction(store, mode);
  }
}

/** Run `fn` on one store and wait for the transaction to finish. */
export async function tx<T>(store: Store, mode: IDBTransactionMode, fn: (s: IDBObjectStore) => IDBRequest<T> | void): Promise<T> {
  const t = await transaction(store, mode);
  return new Promise<T>((resolve, reject) => {
    const req = fn(t.objectStore(store));
    t.oncomplete = () => resolve(req ? req.result : (undefined as T));
    // (A failed request reports its error; an abort without one, this.)
    t.onerror = (e) => reject((e.target as IDBRequest).error);
    t.onabort = () => reject(new Error('transaction aborted'));
  });
}

/** All records of a user in a store. */
export function byUser<T>(store: Store, user: number): Promise<T[]> {
  return tx<T[]>(store, 'readonly', (s) => s.index('user').getAll(user) as IDBRequest<T[]>);
}

/** Delete the whole store (sign-out). */
export function wipe(): Promise<void> {
  const closing = opening;
  opening = null;
  return new Promise<void>((resolve) => {
    void closing?.then((d) => d.close(), () => {});
    const req = indexedDB.deleteDatabase(DB);
    // A tab that ignores versionchange could block it: don't hang sign-out.
    const timer = setTimeout(resolve, 3000);
    req.onsuccess = req.onerror = () => {
      clearTimeout(timer);
      resolve();
    };
  });
}
