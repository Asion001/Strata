// The files page's state and actions: the listing of one folder, the
// selection, the open viewer/editor/details, and what can be done to
// entries. Components get this controller and never call the API.
// (Bulk actions live in ./bulk, loaded on first use.)
import { goto } from '$app/navigation';
import { untrack } from 'svelte';
import { api, contentUrl, previewUrl, thumbUrl, type Entry } from '$lib/api';
import { join, parent, isImage, isBrowserImage } from '$lib/format';
import { Selection, loadPref, nextSort, savePref, sortEntries, type Sort, type SortKey } from './listing';
import { enqueue as queueFiles, userRecorded } from '$lib/offline/app.svelte';
import { reloadMounts } from '$lib/mounts.svelte';
import { session } from '$lib/session.svelte';
import { staleApp } from '$lib/lazy';
import { toast, toastError } from '$lib/toast.svelte';

const Recent = () => import('$lib/offline/recent');
// (A part that can't load, say after a deploy, goes to staleApp, which
// reloads into the new version or says so; the action does nothing.)
const lazy =
  <T>(load: () => Promise<T>) =>
  (): Promise<T | null> =>
    load().catch((e) => {
      void staleApp(e);
      return null;
    });
const Bulk = lazy(() => import('./bulk'));
const Actions = lazy(() => import('./actions'));

export type Mount = { id: number; mode: string; connection_name: string; path: string; offline?: boolean; trash?: boolean };
export type ViewerItem = { name: string; size: number; src: string; download: string; path: string; thumb?: string; poster?: string; mtime: string; etag: string | null };
export type Hit = { path: string; name: string; is_dir: boolean; size: number; mtime: string };

const SORT_KEYS = ['name', 'size', 'mtime'] as const;
// Phones have no column headers to click, so one menu holds every order.
export const SORT_CHOICES: [string, Sort][] = [
  ['Name, A to Z', { key: 'name', desc: false }],
  ['Name, Z to A', { key: 'name', desc: true }],
  ['Newest first', { key: 'mtime', desc: true }],
  ['Oldest first', { key: 'mtime', desc: false }],
  ['Largest first', { key: 'size', desc: true }],
  ['Smallest first', { key: 'size', desc: false }]
];

export const names = (items: Entry[]) => (items.length === 1 ? `“${items[0].name}”` : `${items.length} items`);

export class FilesController {
  readonly #url: () => URL;
  /** The folder asked for (?p=). */
  get path() {
    return this.#url().searchParams.get('p') ?? '';
  }
  entries = $state<Entry[]>([]);
  // The folder `entries` belong to. It lags `path` until a new listing
  // arrives, and every action on an entry joins with it, never with `path`.
  listedPath = $state('');
  mount = $state<Mount | null>(null);
  editing = $state<string | null>(null);
  loading = $state(false);
  error = $state('');
  /** The listing shown came from the offline cache, saved at this time. */
  offlineAt = $state<string | null>(null);
  /** Paths kept offline ("Keep offline"). */
  pinned = $state(new Set<string>());
  query = $state('');
  hits = $state<Hit[] | null>(null);
  menu = $state<Entry | null>(null);
  viewerIndex = $state(-1);
  viewerItems = $state<ViewerItem[]>([]);
  // The details panel, by entry name in the listed folder.
  details = $state<string | null>(null);
  dragging = $state(false);
  // While a bulk action runs, the selection bar and Delete key wait.
  busy = $state(false);

  // Sorting, view mode and selection. Preferences stay in this browser.
  sort = $state<Sort>({
    key: loadPref<SortKey>('strata-sort-key', SORT_KEYS, 'name'),
    desc: loadPref('strata-sort-desc', ['0', '1'] as const, '0') === '1'
  });
  view = $state(loadPref('strata-view', ['list', 'grid'] as const, 'list'));
  readonly selection = new Selection();

  readonly sorted = $derived(sortEntries(this.entries, this.sort));
  readonly order = $derived(this.sorted.map((e) => e.name));
  readonly byName = $derived(new Map(this.entries.map((e) => [e.name, e])));
  // From the selection, not a scan of the folder: cheap in big folders.
  readonly picked = $derived([...this.selection.names].flatMap((n) => this.byName.get(n) ?? []));
  readonly detailsEntry = $derived(this.details === null ? undefined : this.byName.get(this.details));
  readonly crumbs = $derived(this.path ? this.path.split('/').map((n, i, a) => ({ name: n, path: a.slice(0, i + 1).join('/') })) : []);

  #seq = 0;
  // ?f=name opens a file's details once its folder lists (links from ⌘K).
  #appliedF = '';

  /** `url`: the page's URL (reactive). */
  constructor(url: () => URL) {
    this.#url = url;
  }

  /** The page's effects; call once while the page initialises. */
  start() {
    // A new folder starts with nothing selected, both when asked for and when
    // its listing replaces the old one (a pick made on the old rows must not
    // carry over to a same-named file). A refresh keeps what's still there.
    $effect(() => {
      void this.path;
      void this.listedPath;
      this.selection.clear();
    });
    $effect(() => {
      const o = this.order;
      untrack(() => this.selection.prune(o));
    });
    // Gone after a refresh (deleted, renamed elsewhere): close.
    $effect(() => {
      if (this.details !== null && !this.loading && !this.detailsEntry) this.details = null;
    });
    $effect(() => {
      const p = this.path;
      untrack(() => void this.load(p));
    });
    // Live updates: the server pushes the journal cursor; refresh on change.
    $effect(() => {
      let ws: WebSocket | null = null;
      let timer: ReturnType<typeof setTimeout>;
      let closed = false;
      const connect = () => {
        ws = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/ws`);
        let first = true;
        ws.onmessage = () => {
          if (first) return void (first = false);
          clearTimeout(timer);
          // Renames and moves can shift mount paths too, so the sidebar refreshes.
          // A bulk action refreshes once at its end.
          timer = setTimeout(() => this.busy || this.refresh(), 300);
        };
        ws.onclose = () => void (closed || setTimeout(connect, 3000));
      };
      connect();
      return () => {
        closed = true;
        ws?.close();
      };
    });
    // Every file shown in the viewer (opened, or reached with next and
    // previous) goes into the recent-files cache, if it fits, for offline.
    $effect(() => {
      const it = this.viewerIndex >= 0 ? this.viewerItems[this.viewerIndex] : undefined;
      if (!it || !session.user || this.offlineAt) return;
      const user = session.user.id;
      void userRecorded().then(Recent).then((r) => r.remember(user, it.path, it.etag, contentUrl(it.path), it.size)).catch(() => {});
    });
    // Back online: the folder again (and no "offline" banner).
    $effect(() => {
      const back = () => void ((this.offlineAt || this.error) && this.load());
      window.addEventListener('online', back);
      return () => window.removeEventListener('online', back);
    });
    $effect(() => {
      const done = (e: Event) => void (parent((e as CustomEvent<string>).detail) === this.path && this.load());
      window.addEventListener('strata-upload-done', done);
      return () => window.removeEventListener('strata-upload-done', done);
    });
    $effect(() => {
      const q = this.query.trim();
      if (!q) return void (this.hits = null);
      const t = setTimeout(async () => {
        try {
          this.hits = (await api('GET', `/api/search?q=${encodeURIComponent(q)}`)).hits;
        } catch {
          this.hits = [];
        }
      }, 200);
      return () => clearTimeout(t);
    });
  }

  async load(p = this.path) {
    const my = ++this.#seq;
    this.loading = true;
    this.error = '';
    try {
      const meta: { offlineAt?: string | null } = {};
      const r = await api('GET', `/api/files/list?path=${encodeURIComponent(p)}`, undefined, meta);
      // (A stale answer never replaces a newer one.)
      if (my !== this.#seq) return;
      this.offlineAt = meta.offlineAt ?? null;
      void this.loadPins();
      this.entries = r.entries;
      if (this.listedPath !== p) this.details = null;
      this.listedPath = p;
      const u = this.#url();
      const f = u.searchParams.get('f');
      if (f && this.#appliedF !== u.href && (u.searchParams.get('p') ?? '') === p) {
        this.#appliedF = u.href;
        if (r.entries.some((e: Entry) => e.name === f)) this.details = f;
      }
      this.mount = r.mount;
    } catch (e) {
      if (my === this.#seq) {
        this.error = (e as Error).message;
        this.entries = [];
        this.offlineAt = null;
      }
    } finally {
      if (my === this.#seq) this.loading = false;
    }
  }

  /** The folder and the sidebar's mounts again. */
  async refresh() {
    await this.load();
    reloadMounts();
  }

  setSort(s: Sort) {
    this.sort = { ...s };
    savePref('strata-sort-key', s.key);
    savePref('strata-sort-desc', s.desc ? '1' : '0');
  }
  sortBy = (key: SortKey) => this.setSort(nextSort(this.sort, key));
  setView(v: 'list' | 'grid') {
    this.view = v;
    savePref('strata-view', v);
  }

  /** Row/card clicks: modifier keys select, and while anything is selected
   *  a plain click selects too (no keyboard modifiers on phones). Enter on
   *  a name (a click with detail 0) always opens. */
  activate(e: Entry, ev: MouseEvent) {
    if (ev.shiftKey) this.selection.range(e.name, this.order);
    else if (ev.ctrlKey || ev.metaKey || (this.selection.size && ev.detail !== 0)) this.selection.toggle(e.name);
    else this.open(e);
  }
  check(e: Entry, ev: MouseEvent) {
    ev.stopPropagation();
    const box = ev.currentTarget as HTMLInputElement;
    if (ev.shiftKey) this.selection.range(e.name, this.order);
    else this.selection.toggle(e.name);
    // The box flipped itself; the range may say otherwise.
    box.checked = this.selection.has(e.name);
  }

  onKeys = (ev: KeyboardEvent) => {
    // Focus stays on the ⋯ button, outside the menu, so Escape lands here.
    if (this.menu && ev.key === 'Escape') return void (this.menu = null);
    if (this.viewerIndex >= 0 || this.editing || this.menu || this.hits) return;
    if (ev.key === 'Escape' && !this.selection.size && this.details !== null) return void (this.details = null);
    const t = ev.target as HTMLElement | null;
    if (t?.closest('input:not([type=checkbox]), textarea, select, [contenteditable]')) return;
    if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === 'a') {
      ev.preventDefault();
      this.selection.toggleAll(this.order);
    } else if (ev.key === 'Escape' && this.selection.size) this.selection.clear();
    // Delete, or Cmd+Backspace on a Mac (its delete key is Backspace).
    // (With Shift: for good, past the trash.)
    else if ((ev.key === 'Delete' || (ev.key === 'Backspace' && ev.metaKey)) && this.selection.size && !this.busy) {
      ev.preventDefault();
      void this.removeItems(this.picked, true, ev.shiftKey);
    }
  };

  open(e: Entry) {
    const dir = this.listedPath;
    if (e.is_dir) return void goto(`/?p=${encodeURIComponent(join(dir, e.name))}`);
    // A snapshot: a refresh (an upload landing, another device) must not
    // shift which file the open viewer shows.
    const viewable = this.sorted.filter((v) => !v.is_dir);
    this.viewerItems = viewable.map((v) => {
      const p = join(dir, v.name);
      return {
        name: v.name, size: v.size, mtime: v.mtime, path: p, etag: v.etag,
        src: previewUrl(p), download: contentUrl(p, true),
        // The list's own thumbnail URL: already in the cache, so it shows at once.
        thumb: isImage(v.name) ? thumbUrl(p, v.etag) : undefined,
        poster: isImage(v.name) && !isBrowserImage(v.name) ? thumbUrl(p, v.etag, 1024) : undefined
      };
    });
    this.viewerIndex = viewable.indexOf(e);
  }

  showDetails(e: Entry) {
    this.menu = null;
    this.details = e.name;
  }
  edit(e: Entry) {
    this.editing = join(this.listedPath, e.name);
    this.menu = null;
  }

  async act(fn: () => Promise<unknown>) {
    try {
      await fn();
    } catch (e) {
      toastError(e);
    }
    this.menu = null;
    await this.refresh();
  }

  // Entry actions (their code loads on first use).
  newFolder = async () => (await Actions())?.newFolder(this);
  rename = async (e: Entry) => (await Actions())?.rename(this, e);
  moveTo = async (e: Entry, copy = false) => (await Actions())?.moveTo(this, e, copy);
  duplicate = async (e: Entry) => (await Actions())?.duplicate(this, e);
  share = async (e: Entry) => (await Actions())?.share(this, e);
  togglePin = async (e: Entry) => (await Actions())?.togglePin(this, e);
  remove(e: Entry, permanent = false) {
    this.menu = null;
    void this.removeItems([e], false, permanent);
  }

  async loadPins() {
    if (!session.user) return;
    try {
      this.pinned = new Set((await (await Recent()).list(session.user.id)).filter((r) => r.pinned).map((r) => r.path));
    } catch {}
  }
  // Uploads go through the offline queue (they survive going offline, a
  // reload or a closed tab); a finished one refreshes its folder.
  enqueue(files: Iterable<File>, base = this.path) {
    if (!session.user) return;
    const items = [...files].map((f) => ({ file: f, path: join(base, (f as File & { webkitRelativePath?: string }).webkitRelativePath || f.name) }));
    queueFiles(session.user.id, items).catch((e) => toastError(e));
  }
  onDrop = (e: DragEvent) => {
    e.preventDefault();
    this.dragging = false;
    if (e.dataTransfer?.files.length) this.enqueue(e.dataTransfer.files);
  };

  waitForBusy = () => toast('Wait for the current action to finish.', 'info');

  // Bulk actions (their code loads on first use).
  /** The names that failed; null when nothing was tried (busy, cancelled).
   *  `permanent` skips the trash (Shift+Delete, "Delete permanently"). */
  removeItems = async (all: Entry[], fromSelection = true, permanent = false) => (await Bulk())?.removeItems(this, all, fromSelection, permanent) ?? null;
  transferPicked = async (copy: boolean) => (await Bulk())?.transferPicked(this, copy);

  /** Delete from the viewer: on to the next file, or closed after the last. */
  viewerDelete = async (i: number) => {
    const it = this.viewerItems[i];
    const e = this.byName.get(it.name);
    if (!e) return void toast(`“${it.name}” is no longer in this folder.`, 'info');
    const failed = await this.removeItems([e], false);
    if (!failed || failed.length) return; // cancelled, busy or failed
    // (By path: the viewer may have been closed, or reopened on a newer
    // snapshot, while the dialog and the delete ran.)
    const at = this.viewerItems.findIndex((x) => x.path === it.path);
    if (at < 0) return;
    const open = this.viewerIndex >= 0;
    const current = this.viewerItems[this.viewerIndex]?.path;
    this.viewerItems = this.viewerItems.filter((x) => x.path !== it.path);
    if (!open) return;
    const still = this.viewerItems.findIndex((x) => x.path === current);
    this.viewerIndex = !this.viewerItems.length ? -1 : still >= 0 ? still : Math.min(at, this.viewerItems.length - 1);
  };
}
