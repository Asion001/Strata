// Short, non-blocking messages shown by Toaster, which the root layout
// renders once it has loaded (on the first toast, to keep it out of the
// startup bundle).
import type { Component } from 'svelte';

/** A button on a toast (such as Undo): pressing it runs `run` and closes the toast. */
export type ToastAction = { label: string; run: () => void };
export type Toast = { id: number; kind: 'info' | 'success' | 'error'; text: string; action?: ToastAction };

export const toasts = $state<{ list: Toast[] }>({ list: [] });

export const toaster = $state<{ component: Component | null }>({ component: null });
let loading: Promise<void> | null = null;

function loadToaster() {
  loading ??= import('$lib/components/ui/Toaster.svelte').then(
    (m) => void (toaster.component = m.default),
    (err) => {
      // Can't load (offline after an update): say what was pending anyway.
      console.error('could not load the toast component', err);
      loading = null;
      const pending = toasts.list.splice(0);
      if (pending.length) window.alert(pending.map((t) => t.text).join('\n\n'));
    }
  );
}

let next = 0;
const MAX = 4;

export function dismissToast(id: number) {
  const i = toasts.list.findIndex((t) => t.id === id);
  if (i >= 0) toasts.list.splice(i, 1);
}

/** Shows a message; errors stay longer. Returns its id. */
export function toast(text: string, kind: Toast['kind'] = 'info', ms = kind === 'error' ? 8000 : 4000, action?: ToastAction): number {
  const id = ++next;
  const t: Toast = { id, kind, text, action };
  toasts.list.push(t);
  if (!toaster.component) loadToaster();
  if (toasts.list.length > MAX) toasts.list.splice(0, toasts.list.length - MAX);
  if (ms > 0) setTimeout(() => dismissToast(id), ms);
  return id;
}

/** Runs a toast's action once and closes the toast. */
export function runToastAction(id: number) {
  const t = toasts.list.find((x) => x.id === id);
  if (!t?.action) return;
  dismissToast(id);
  t.action.run();
}

/** Shows an error's message (or a fallback for non-Error values). */
export function toastError(err: unknown) {
  const msg = err instanceof Error ? err.message : typeof err === 'string' ? err : '';
  return toast(msg || 'Something went wrong.', 'error');
}
