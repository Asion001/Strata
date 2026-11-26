// An in-memory Cache Storage for unit tests (Node has none).

const keyOf = (r: RequestInfo | URL) => (typeof r === 'string' ? r : r instanceof URL ? r.href : r.url);

export class MemoryCache {
  entries = new Map<string, Response>();
  async put(req: RequestInfo | URL, res: Response) {
    this.entries.set(keyOf(req), new Response(await res.arrayBuffer(), { status: res.status, headers: res.headers }));
  }
  async match(req: RequestInfo | URL) {
    return this.entries.get(keyOf(req))?.clone();
  }
  async delete(req: RequestInfo | URL) {
    return this.entries.delete(keyOf(req));
  }
}

export class MemoryCacheStorage {
  stores = new Map<string, MemoryCache>();
  async open(name: string) {
    let c = this.stores.get(name);
    if (!c) this.stores.set(name, (c = new MemoryCache()));
    return c as unknown as Cache;
  }
  async keys() {
    return [...this.stores.keys()];
  }
  async delete(name: string) {
    return this.stores.delete(name);
  }
  async has(name: string) {
    return this.stores.has(name);
  }
  async match(req: RequestInfo | URL, opts?: { cacheName?: string }) {
    const names = opts?.cacheName ? [opts.cacheName] : [...this.stores.keys()];
    for (const n of names) {
      const hit = await this.stores.get(n)?.match(req);
      if (hit) return hit;
    }
    return undefined;
  }
}
