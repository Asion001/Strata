// In-app replacements for window.confirm/prompt/alert. Each call returns a
// promise; requests queue up and DialogHost (in the root layout) shows them
// one at a time.
import type { Component } from 'svelte';
import type { PickRules } from '$lib/components/files/folder-picker';

export type Field = {
  name: string;
  label: string;
  value?: string;
  type?: 'text' | 'password' | 'number';
  placeholder?: string;
  hint?: string;
  /** Submit stays disabled while this field is blank (after trimming). */
  required?: boolean;
  min?: number;
  /** Defaults to new-password for password fields, off for the rest. */
  autocomplete?: AutoFill;
};

type Common = {
  title: string;
  body?: string;
  /** Withdraws the dialog (answered as a cancel), e.g. when its owner unmounts. */
  signal?: AbortSignal;
};
export type ConfirmOpts = Common & { confirm?: string; cancel?: string; danger?: boolean };
export type FormOpts = Common & { fields: Field[]; confirm?: string; cancel?: string };
export type AlertOpts = Common & { ok?: string; /** Shown read-only with a Copy button. */ copy?: string };
export type FolderOpts = Common & PickRules & { confirm?: string; /** The folder it opens at. */ start: string };

export type Request = { id: number } & (
  | { kind: 'confirm'; opts: ConfirmOpts; done: (v: boolean) => void }
  | { kind: 'form'; opts: FormOpts; done: (v: Record<string, string> | null) => void }
  | { kind: 'alert'; opts: AlertOpts; done: () => void }
  | { kind: 'folder'; opts: FolderOpts; done: (v: string | null) => void }
);

export const dialogs = $state<{ queue: Request[] }>({ queue: [] });

/** The host component, loaded on first use to keep it out of the startup
 *  bundle. The root layout renders it once it is here. */
export const dialogHost = $state<{ component: Component | null }>({ component: null });
let loading: Promise<void> | null = null;

function loadHost() {
  loading ??= import('$lib/components/ui/DialogHost.svelte').then(
    (m) => void (dialogHost.component = m.default),
    (err) => {
      // Can't load (offline after an update): fall back to the browser's
      // own dialogs so nothing waits forever. Try again next time.
      console.error('could not load the dialog component', err);
      loading = null;
      for (const r of dialogs.queue.splice(0)) native(r);
    }
  );
}

function native(r: Request) {
  const text = r.opts.body ? `${r.opts.title}\n\n${r.opts.body}` : r.opts.title;
  if (r.kind === 'confirm') r.done(window.confirm(text));
  else if (r.kind === 'folder') {
    // (The rules stay with the picker, out of the startup bundle: the
    // server refuses a move into itself, and the same folder does nothing.)
    const v = window.prompt(`${text}\n\nTo folder (full path, empty for Home)`, r.opts.start);
    r.done(v === null ? null : v.trim().replace(/^\/+|\/+$/g, ''));
  } else if (r.kind === 'alert') {
    window.alert(r.opts.copy ? `${text}\n\n${r.opts.copy}` : text);
    r.done();
  } else {
    // A password typed into window.prompt would show in clear text: skip
    // optional ones (left empty), refuse when one is needed.
    if (r.opts.fields.some((f) => f.type === 'password' && f.required)) {
      window.alert('This part of the app could not load. Reload the page and try again.');
      return r.done(null);
    }
    const out: Record<string, string> = {};
    const skips = r.opts.fields.some((f) => f.type === 'password');
    if (skips && !window.confirm(`${text}\n\nPart of the app could not load, so a password can't be entered safely. Continue without one?`))
      return r.done(null);
    for (const f of r.opts.fields) {
      if (f.type === 'password') {
        out[f.name] = '';
        continue;
      }
      const v = window.prompt(`${text}\n\n${f.label}`, f.value ?? '');
      if (v === null) return r.done(null);
      const n = Number(v);
      const bad =
        (f.required && !v.trim()) ||
        (f.type === 'number' && v !== '' && !(Number.isFinite(n) && (f.min === undefined || n >= f.min)));
      if (bad) {
        window.alert(`“${v}” isn't a valid ${f.label.toLowerCase()}.`);
        return r.done(null);
      }
      out[f.name] = v;
    }
    r.done(out);
  }
}

/** Answers a request the way Cancel does. */
export function cancelRequest(r: Request | undefined) {
  if (r?.kind === 'confirm') r.done(false);
  else if (r?.kind === 'form' || r?.kind === 'folder') r.done(null);
  else r?.done();
}

/** Cancels everything queued, e.g. after the page changed underneath. */
export function cancelAllDialogs() {
  for (const r of [...dialogs.queue]) cancelRequest(r);
}

let nextId = 0;
function push<T>(make: (done: (v: T) => void) => Omit<Request, 'id'>): Promise<T> {
  return new Promise<T>((resolve) => {
    const id = ++nextId;
    let settled = false;
    let cleanup = () => {};
    const req = make((v) => {
      if (settled) return;
      settled = true;
      cleanup();
      // By id: the queue holds reactive proxies, not this object.
      const i = dialogs.queue.findIndex((r) => r.id === id);
      if (i >= 0) dialogs.queue.splice(i, 1);
      resolve(v);
    });
    dialogs.queue.push({ ...req, id } as Request);
    const signal = req.opts.signal;
    if (signal?.aborted) return cancelRequest(dialogs.queue[dialogs.queue.length - 1]);
    if (signal) {
      const withdraw = () => cancelRequest(dialogs.queue.find((q) => q.id === id));
      signal.addEventListener('abort', withdraw, { once: true });
      cleanup = () => signal.removeEventListener('abort', withdraw);
    }
    if (!dialogHost.component) loadHost();
  });
}

/** Resolves true on confirm, false on cancel, Escape or a click outside. */
export function confirmDialog(opts: ConfirmOpts): Promise<boolean> {
  return push((done) => ({ kind: 'confirm', opts, done }));
}

/** Resolves to the field values by name, or null when cancelled. */
export function formDialog(opts: FormOpts): Promise<Record<string, string> | null> {
  return push((done) => ({ kind: 'form', opts, done }));
}

/** One text field (required unless said otherwise); null when cancelled. */
export async function promptDialog(
  opts: Common & Pick<Field, 'label' | 'value' | 'type' | 'required' | 'autocomplete'> & { confirm?: string }
): Promise<string | null> {
  const r = await formDialog({
    title: opts.title,
    body: opts.body,
    signal: opts.signal,
    confirm: opts.confirm,
    fields: [{ name: 'v', label: opts.label, value: opts.value, type: opts.type, autocomplete: opts.autocomplete, required: opts.required ?? true }]
  });
  return r ? r.v : null;
}

/** A folder chosen in a folder browser ("" is Home); null when cancelled. */
export function pickFolder(opts: FolderOpts): Promise<string | null> {
  return push((done) => ({ kind: 'folder', opts, done }));
}

export function alertDialog(opts: AlertOpts): Promise<void> {
  return push((done) => ({ kind: 'alert', opts, done }));
}
