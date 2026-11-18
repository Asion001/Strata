// In-app replacements for window.confirm/prompt/alert. Each call returns a
// promise; requests queue up and DialogHost (in the root layout) shows them
// one at a time.
import type { Component } from 'svelte';
import { staleApp } from '$lib/lazy';

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
};

type Common = { title: string; body?: string };
export type ConfirmOpts = Common & { confirm?: string; cancel?: string; danger?: boolean };
export type FormOpts = Common & { fields: Field[]; confirm?: string; cancel?: string };
export type AlertOpts = Common & { ok?: string; /** Shown read-only with a Copy button. */ copy?: string };

export type Request = { id: number } & (
  | { kind: 'confirm'; opts: ConfirmOpts; done: (v: boolean) => void }
  | { kind: 'form'; opts: FormOpts; done: (v: Record<string, string> | null) => void }
  | { kind: 'alert'; opts: AlertOpts; done: () => void }
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
      // Maybe the app was updated and its old chunks are gone: reload then.
      void staleApp(err);
    }
  );
}

function native(r: Request) {
  const text = r.opts.body ? `${r.opts.title}\n\n${r.opts.body}` : r.opts.title;
  if (r.kind === 'confirm') r.done(window.confirm(text));
  else if (r.kind === 'alert') {
    window.alert(r.opts.copy ? `${text}\n\n${r.opts.copy}` : text);
    r.done();
  } else {
    const out: Record<string, string> = {};
    for (const f of r.opts.fields) {
      const v = window.prompt(`${text}\n\n${f.label}`, f.value ?? '');
      if (v === null) return r.done(null);
      out[f.name] = v;
    }
    r.done(out);
  }
}

let nextId = 0;
function push<T>(make: (done: (v: T) => void) => Omit<Request, 'id'>): Promise<T> {
  return new Promise<T>((resolve) => {
    const id = ++nextId;
    let settled = false;
    const req = make((v) => {
      if (settled) return;
      settled = true;
      // By id: the queue holds reactive proxies, not this object.
      const i = dialogs.queue.findIndex((r) => r.id === id);
      if (i >= 0) dialogs.queue.splice(i, 1);
      resolve(v);
    });
    dialogs.queue.push({ ...req, id } as Request);
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
  opts: Common & { label: string; value?: string; confirm?: string; type?: Field['type']; required?: boolean }
): Promise<string | null> {
  const r = await formDialog({
    title: opts.title,
    body: opts.body,
    confirm: opts.confirm,
    fields: [{ name: 'v', label: opts.label, value: opts.value, type: opts.type, required: opts.required ?? true }]
  });
  return r ? r.v : null;
}

export function alertDialog(opts: AlertOpts): Promise<void> {
  return push((done) => ({ kind: 'alert', opts, done }));
}
