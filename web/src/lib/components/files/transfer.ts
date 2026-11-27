// Move and copy with name conflicts: when a name is taken, the user picks
// Replace (Merge, for a folder onto a folder), Keep both or Skip, once for
// the whole batch if they like. No dialog or network code of its own: both
// come in, so the logic is tested on its own.
import { ApiError, type Entry, type Existing } from '$lib/api';
import { join } from '$lib/format';
import type { Choice, ConflictOpts, Side } from '$lib/dialogs.svelte';

export type Ask = (o: ConflictOpts) => Promise<Choice | null>;
export type Call = <T = any>(method: string, url: string, body?: unknown) => Promise<T>;
export type Mode = 'fail' | 'replace' | 'rename';
type Decision = Choice['action'] | 'stop';

/** Cancelled from a conflict dialog: nothing more is done. */
export class Stopped extends Error {
  constructor() {
    super('stopped');
  }
}

/** The answers of one batch: asks, and remembers "apply to all". */
export class Batch {
  #sticky: Choice['action'] | null = null;
  #stopped = false;
  /** Conflicts known to be still ahead (for "apply to the other N"). */
  ahead = 0;
  constructor(private ask: Ask) {}

  get stopped() {
    return this.#stopped;
  }

  async decide(name: string, existing: Side, incoming: Side): Promise<Decision> {
    if (this.#stopped) return 'stop';
    if (this.ahead > 0) this.ahead--;
    // (A file and a folder can't replace each other: that one asks.)
    const canReplace = existing.is_dir === incoming.is_dir;
    if (this.#sticky && (this.#sticky !== 'replace' || canReplace)) return this.#sticky;
    const kind = existing.is_dir ? 'A folder' : 'A file';
    const c = await this.ask({ title: `${kind} named “${name}” is already there`, name, existing, incoming, remaining: this.ahead });
    if (!c) {
      this.#stopped = true;
      return 'stop';
    }
    if (c.all) this.#sticky = c.action;
    return c.action;
  }
}

/** What a batch did, by top-level name. */
export type Tally = {
  done: string[];
  skipped: string[];
  failed: { name: string; error: string }[];
  /** Kept both: name -> the path it got. */
  renamed: Map<string, string>;
};

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));
const listing = async (call: Call, path: string): Promise<Entry[]> =>
  (await call<{ entries: Entry[] }>('GET', `/api/files/list?path=${encodeURIComponent(path)}`)).entries;

export class Transfer {
  constructor(
    private call: Call,
    private batch: Batch,
    private copy: boolean
  ) {}

  /** Move or copy `items` of `fromDir` into `toDir`, each under its name. */
  async all(items: Pick<Entry, 'name' | 'is_dir' | 'size' | 'mtime'>[], fromDir: string, toDir: string): Promise<Tally> {
    const t: Tally = { done: [], skipped: [], failed: [], renamed: new Map() };
    await this.#countAhead(items.map((i) => i.name), toDir);
    for (const item of items) {
      if (this.batch.stopped) {
        t.skipped.push(item.name);
        continue;
      }
      try {
        const r = await this.one(item, join(fromDir, item.name), join(toDir, item.name));
        if (r === 'skipped') t.skipped.push(item.name);
        else {
          t.done.push(item.name);
          if (r.renamed) t.renamed.set(item.name, r.path);
        }
      } catch (e) {
        if (e instanceof Stopped) t.skipped.push(item.name);
        else t.failed.push({ name: item.name, error: message(e) });
      }
    }
    return t;
  }

  /** One item; a folder merged into another goes item by item. */
  async one(item: Side, from: string, to: string, mode: Mode = 'fail'): Promise<{ path: string; renamed: boolean } | 'skipped'> {
    try {
      const r = await this.call<{ path: string }>('POST', this.copy ? '/api/files/copy' : '/api/files/move', { from, to, on_conflict: mode });
      return { path: r.path, renamed: r.path !== to };
    } catch (e) {
      if (!(e instanceof ApiError && e.status === 409 && e.code === 'exists' && mode === 'fail')) throw e;
      const there: Existing | Side = e.details ?? { is_dir: false };
      const name = to.slice(to.lastIndexOf('/') + 1);
      const d = await this.batch.decide(name, there, item);
      if (d === 'stop') throw new Stopped();
      if (d === 'skip') return 'skipped';
      if (d === 'rename') return this.one(item, from, to, 'rename');
      if (item.is_dir && there.is_dir) {
        // (What it skipped stays where it was, with its folder.)
        await this.#merge(from, to);
        return { path: to, renamed: false };
      }
      return this.one(item, from, to, 'replace');
    }
  }

  /** Put what is in folder `from` into folder `to`, asking per name taken.
   *  A move removes `from` once it is empty. Whether all of it went. */
  async #merge(from: string, to: string): Promise<boolean> {
    const inside = await listing(this.call, from);
    await this.#countAhead(
      inside.map((e) => e.name),
      to
    );
    let whole = true;
    for (const e of inside) {
      // (A mount inside stays: mounts move on the Mounts page.)
      if (e.mount_id) {
        whole = false;
        continue;
      }
      const r = await this.one(e, join(from, e.name), join(to, e.name));
      if (r === 'skipped') whole = false;
    }
    // (Only when nothing was left behind, and nothing came in meanwhile.)
    if (whole && !this.copy && !(await listing(this.call, from)).length) {
      await this.call('POST', '/api/files/delete', { paths: [from] });
    }
    return whole;
  }

  /** How many of `names` are taken in `dir` (best effort). */
  async #countAhead(names: string[], dir: string) {
    try {
      const there = new Set((await listing(this.call, dir)).map((e) => e.name));
      this.batch.ahead += names.filter((n) => there.has(n)).length;
    } catch {
      // (Not listed: "apply to all" shows without a count.)
    }
  }
}
