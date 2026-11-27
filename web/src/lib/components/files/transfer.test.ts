import { describe, expect, it, vi } from 'vitest';
import { ApiError } from '$lib/api';
import type { Choice, ConflictOpts } from '$lib/dialogs.svelte';
import { Batch, Stopped, Transfer, type Call } from './transfer';

type Node = { dir: true; kids: Map<string, Node> } | { dir: false; data: string };

/** A file tree behind the move, copy, list and delete calls, answering as
 *  the server does about taken names. */
class Tree {
  root: Node = { dir: true, kids: new Map() };
  calls: string[] = [];
  /** Requests that fail, by path. */
  broken = new Set<string>();

  add(path: string, data?: string) {
    const parts = path.split('/');
    let n = this.root as Extract<Node, { dir: true }>;
    for (const [i, p] of parts.entries()) {
      const last = i === parts.length - 1;
      if (!n.kids.has(p)) n.kids.set(p, last && data !== undefined ? { dir: false, data } : { dir: true, kids: new Map() });
      n = n.kids.get(p) as Extract<Node, { dir: true }>;
    }
    return this;
  }
  get(path: string): Node | undefined {
    let n: Node | undefined = this.root;
    for (const p of path ? path.split('/') : []) n = n?.dir ? n.kids.get(p) : undefined;
    return n;
  }
  #parent(path: string) {
    const i = path.lastIndexOf('/');
    return [this.get(i < 0 ? '' : path.slice(0, i)) as Extract<Node, { dir: true }>, path.slice(i + 1)] as const;
  }
  dump(): Record<string, string> {
    const out: Record<string, string> = {};
    const walk = (n: Node, p: string) => {
      if (!n.dir) out[p] = n.data;
      else {
        if (p && !n.kids.size) out[p] = '<dir>';
        for (const [k, v] of n.kids) walk(v, p ? `${p}/${k}` : k);
      }
    };
    walk(this.root, '');
    return out;
  }

  call = vi.fn(async (method: string, url: string, body?: any): Promise<any> => {
    this.calls.push(`${method} ${url}${body ? ' ' + JSON.stringify(body) : ''}`);
    if (url.startsWith('/api/files/list')) {
      const path = decodeURIComponent(url.split('path=')[1]);
      if (this.broken.has(`list:${path}`)) throw new ApiError(503, 'offline');
      const n = this.get(path);
      if (!n?.dir) throw new ApiError(404, 'not found');
      return { entries: [...n.kids].map(([name, k]) => ({ name, is_dir: k.dir, size: k.dir ? 0 : k.data.length, mtime: '2026-01-01T00:00:00Z', mount_id: name.startsWith('mnt') ? 1 : null })) };
    }
    if (url === '/api/files/delete') {
      const [dir, name] = this.#parent(body.paths[0]);
      dir.kids.delete(name);
      return { results: [] };
    }
    const copy = url === '/api/files/copy';
    const { from, to, on_conflict: mode } = body;
    if (this.broken.has(from)) throw new ApiError(500, `can't ${copy ? 'copy' : 'move'} ${from}`);
    const src = this.get(from)!;
    let [dir, name] = this.#parent(to);
    const there = dir.kids.get(name);
    if (there) {
      if (mode === 'fail') {
        const d = { name, path: to, is_dir: there.dir, size: there.dir ? 0 : there.data.length, mtime: '2026-01-01T00:00:00Z' };
        throw new ApiError(409, 'exists', 'exists', d);
      }
      if (mode === 'rename') {
        let n = 2;
        while (dir.kids.has(`${name} (${n})`)) n++;
        name = `${name} (${n})`;
      } else if (there.dir !== src.dir) throw new ApiError(409, 'a file can’t replace a folder');
    }
    const clone = (n: Node): Node => (n.dir ? { dir: true, kids: new Map([...n.kids].map(([k, v]) => [k, clone(v)])) } : { ...n });
    dir.kids.set(name, copy ? clone(src) : src);
    if (!copy) {
      const [sd, sn] = this.#parent(from);
      sd.kids.delete(sn);
    }
    const at = to.slice(0, to.lastIndexOf('/') + 1) + name;
    return { path: at };
  });
}

/** Answers the dialogs in turn; records what each asked. */
function answers(...list: (Choice | null)[]) {
  const asked: ConflictOpts[] = [];
  const ask = vi.fn(async (o: ConflictOpts) => {
    asked.push(o);
    return list.length ? list.shift()! : null;
  });
  return { ask, asked };
}
const entry = (t: Tree, path: string) => {
  const n = t.get(path)!;
  return { name: path.slice(path.lastIndexOf('/') + 1), is_dir: n.dir, size: n.dir ? 0 : n.data.length, mtime: '2026-02-02T00:00:00Z' };
};
const run = (t: Tree, ask: ReturnType<typeof answers>['ask'], copy: boolean, from: string, to: string, names: string[]) =>
  new Transfer(t.call, new Batch(ask), copy).all(
    names.map((n) => entry(t, `${from}/${n}`)),
    from,
    to
  );

describe('move and copy with taken names', () => {
  it('a free name goes without asking', async () => {
    const t = new Tree().add('a/x.txt', 'x').add('b');
    const { ask } = answers();
    const r = await run(t, ask, false, 'a', 'b', ['x.txt']);
    expect(r).toMatchObject({ done: ['x.txt'], skipped: [], failed: [] });
    expect(t.dump()).toEqual({ a: '<dir>', 'b/x.txt': 'x' });
    expect(ask).not.toHaveBeenCalled();
  });

  it('asks with both sides, and replaces, keeps both or skips', async () => {
    const t = new Tree().add('a/1', 'new1').add('a/2', 'new2').add('a/3', 'new3').add('b/1', 'o').add('b/2', 'o').add('b/3', 'o');
    const { ask, asked } = answers({ action: 'replace', all: false }, { action: 'rename', all: false }, { action: 'skip', all: false });
    const r = await run(t, ask, false, 'a', 'b', ['1', '2', '3']);
    expect(r.done).toEqual(['1', '2']);
    expect(r.skipped).toEqual(['3']);
    expect([...r.renamed]).toEqual([['2', 'b/2 (2)']]);
    expect(t.dump()).toEqual({ 'a/3': 'new3', 'b/1': 'new1', 'b/2': 'o', 'b/2 (2)': 'new2', 'b/3': 'o' });
    expect(asked[0]).toMatchObject({ name: '1', existing: { is_dir: false, size: 1 }, incoming: { is_dir: false, size: 4 }, remaining: 2 });
    expect(asked[0].title).toBe('A file named “1” is already there');
    expect(asked.map((a) => a.remaining)).toEqual([2, 1, 0]);
  });

  it('"apply to all" answers the rest of the batch', async () => {
    const t = new Tree().add('a/1', 'n').add('a/2', 'n').add('a/3', 'n').add('b/1', 'o').add('b/3', 'o');
    const { ask } = answers({ action: 'rename', all: true });
    const r = await run(t, ask, true, 'a', 'b', ['1', '2', '3']);
    expect(ask).toHaveBeenCalledTimes(1);
    expect(r.done).toEqual(['1', '2', '3']);
    expect(Object.keys(t.dump()).filter((k) => k.startsWith('b/'))).toEqual(['b/1', 'b/3', 'b/1 (2)', 'b/2', 'b/3 (2)']);
  });

  it('cancelling stops the batch: the rest is left alone', async () => {
    const t = new Tree().add('a/1', 'n').add('a/2', 'n').add('b/1', 'o').add('b/2', 'o');
    const { ask } = answers(null);
    const r = await run(t, ask, false, 'a', 'b', ['1', '2']);
    expect(r).toMatchObject({ done: [], skipped: ['1', '2'] });
    expect(ask).toHaveBeenCalledTimes(1);
    expect(t.dump()).toEqual({ 'a/1': 'n', 'a/2': 'n', 'b/1': 'o', 'b/2': 'o' });
  });

  it('a failure is reported and the batch goes on', async () => {
    const t = new Tree().add('a/1', 'n').add('a/2', 'n').add('b');
    t.broken.add('a/1');
    const { ask } = answers();
    const r = await run(t, ask, false, 'a', 'b', ['1', '2']);
    expect(r.failed).toEqual([{ name: '1', error: "can't move a/1" }]);
    expect(r.done).toEqual(['2']);
  });

  it('a file and a folder never replace each other, even with "apply to all"', async () => {
    const t = new Tree().add('a/x', 'file').add('a/y', 'n').add('b/x/inner', 'keep').add('b/y', 'o');
    const { ask, asked } = answers({ action: 'replace', all: true }, { action: 'rename', all: false });
    const r = await run(t, ask, false, 'a', 'b', ['y', 'x']);
    // y: replaced (it applies to all); x: a file onto a folder asks again.
    expect(asked.map((a) => a.name)).toEqual(['y', 'x']);
    expect(asked[1].title).toBe('A folder named “x” is already there');
    expect(r.done).toEqual(['y', 'x']);
    expect(t.dump()).toMatchObject({ 'b/x/inner': 'keep', 'b/x (2)': 'file', 'b/y': 'n' });
  });
});

describe('merging a folder into one of its name', () => {
  const tree = () =>
    new Tree()
      .add('a/d/same', 'new')
      .add('a/d/only-new', 'n')
      .add('a/d/sub/deep', 'n2')
      .add('b/d/same', 'old')
      .add('b/d/only-old', 'o')
      .add('b/d/sub/deeper', 'o2');

  it('a move puts what is inside into it, asking per name, then removes the empty folder', async () => {
    const t = tree();
    const { ask, asked } = answers({ action: 'replace', all: false }, { action: 'replace', all: false }, { action: 'replace', all: false });
    const r = await run(t, ask, false, 'a', 'b', ['d']);
    expect(r.done).toEqual(['d']);
    expect(asked.map((a) => [a.name, a.existing.is_dir])).toEqual([
      ['d', true],
      ['same', false],
      ['sub', true]
    ]);
    expect(t.dump()).toEqual({ a: '<dir>', 'b/d/same': 'new', 'b/d/only-old': 'o', 'b/d/sub/deeper': 'o2', 'b/d/sub/deep': 'n2', 'b/d/only-new': 'n' });
    expect(t.calls.filter((c) => c.includes('delete'))).toEqual(['POST /api/files/delete {"paths":["a/d/sub"]}', 'POST /api/files/delete {"paths":["a/d"]}']);
  });

  it('a copy leaves the source as it was', async () => {
    const t = tree();
    const { ask } = answers({ action: 'replace', all: true });
    await run(t, ask, true, 'a', 'b', ['d']);
    expect(t.dump()).toMatchObject({ 'a/d/same': 'new', 'a/d/sub/deep': 'n2', 'b/d/same': 'new', 'b/d/only-old': 'o', 'b/d/sub/deep': 'n2', 'b/d/sub/deeper': 'o2' });
    expect(t.calls.some((c) => c.includes('delete'))).toBe(false);
  });

  it('what is skipped stays where it was, with its folder; a mount inside too', async () => {
    const t = tree().add('a/d/mnt1/x', 'm');
    const { ask } = answers({ action: 'replace', all: false }, { action: 'skip', all: false }, { action: 'replace', all: false });
    const r = await run(t, ask, false, 'a', 'b', ['d']);
    expect(r.done).toEqual(['d']);
    expect(t.dump()).toMatchObject({ 'a/d/same': 'new', 'a/d/mnt1/x': 'm', 'b/d/same': 'old', 'b/d/only-new': 'n' });
    expect(t.get('a/d/sub')).toBeUndefined();
  });

  it('keeping both folders needs no merge', async () => {
    const t = tree();
    const { ask } = answers({ action: 'rename', all: false });
    const r = await run(t, ask, false, 'a', 'b', ['d']);
    expect([...r.renamed]).toEqual([['d', 'b/d (2)']]);
    expect(t.dump()).toMatchObject({ 'b/d (2)/same': 'new', 'b/d/same': 'old' });
  });

  it('cancelled midway: what was done stays done, the rest stays put', async () => {
    const t = tree();
    const { ask } = answers({ action: 'replace', all: false }, null);
    const r = await run(t, ask, false, 'a', 'b', ['d']);
    expect(r.skipped).toEqual(['d']);
    // (Cancelled at the first name inside: nothing after it moved.)
    expect(t.dump()).toEqual({ 'a/d/same': 'new', 'a/d/only-new': 'n', 'a/d/sub/deep': 'n2', 'b/d/same': 'old', 'b/d/only-old': 'o', 'b/d/sub/deeper': 'o2' });
  });

  it('without a listing of the destination, it asks all the same (no count)', async () => {
    const t = new Tree().add('a/1', 'n').add('b/1', 'o');
    t.broken.add('list:b');
    const { ask, asked } = answers({ action: 'skip', all: false });
    await run(t, ask, false, 'a', 'b', ['1']);
    expect(asked[0].remaining).toBe(0);
  });
});

describe('one item', () => {
  it('says when it kept both, and stops on cancel', async () => {
    const t = new Tree().add('a/x', 'n').add('b/x', 'o');
    const { ask } = answers({ action: 'rename', all: false });
    const tr = new Transfer(t.call, new Batch(ask), false);
    expect(await tr.one(entry(t, 'a/x'), 'a/x', 'b/x')).toEqual({ path: 'b/x (2)', renamed: true });
    t.add('a/y', 'n').add('b/y', 'o');
    const stop = new Transfer(t.call, new Batch(answers(null).ask), false);
    await expect(stop.one(entry(t, 'a/y'), 'a/y', 'b/y')).rejects.toBeInstanceOf(Stopped);
    // A stopped batch asks no more.
    await expect(stop.one(entry(t, 'a/y'), 'a/y', 'b/y')).rejects.toBeInstanceOf(Stopped);
  });

  it('a conflict with nothing said about what is there is taken for a file', async () => {
    const call = vi.fn(async (_m: string, _u: string, b: any) => {
      if (b.on_conflict === 'fail') throw new ApiError(409, 'exists', 'exists', null);
      return { path: b.to };
    }) as unknown as Call;
    const { ask, asked } = answers({ action: 'replace', all: false });
    expect(await new Transfer(call, new Batch(ask), true).one({ is_dir: false }, 'a', 'b')).toEqual({ path: 'b', renamed: false });
    expect(asked[0].existing).toEqual({ is_dir: false });
  });

  it('a batch reports a failure that is not an Error too', async () => {
    const call = vi.fn(async (m: string) => {
      if (m === 'GET') return { entries: [] };
      throw 'no network';
    }) as unknown as Call;
    const r = await new Transfer(call, new Batch(answers().ask), false).all([{ name: 'x', is_dir: false, size: 1, mtime: '' }], 'a', 'b');
    expect(r.failed).toEqual([{ name: 'x', error: 'no network' }]);
  });

  it('other errors pass through, a 409 of another kind too', async () => {
    const call = vi.fn(async () => {
      throw new ApiError(409, 'something is mounted at the destination');
    });
    await expect(new Transfer(call, new Batch(answers().ask), false).one({ is_dir: false }, 'a', 'b')).rejects.toThrow('mounted');
  });
});
