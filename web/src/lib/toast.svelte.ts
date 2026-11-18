// Short, non-blocking messages shown by Toaster, which the root layout
// renders once it has loaded (on the first toast, to keep it out of the
// startup bundle).
import type { Component } from 'svelte';

export type Toast = { id: number; kind: 'info' | 'success' | 'error'; text: string };

export const toasts = $state<{ list: Toast[] }>({ list: [] });

export const toaster = $state<{ component: Component | null }>({ component: null });
let loading: Promise<void> | null = null;

function loadToaster(first: Toast) {
  loading ??= import('$lib/components/ui/Toaster.svelte').then(
    (m) => void (toaster.component = m.default),
    (err) => {
      // Can't load (offline after an update): errors still need saying.
      console.error('could not load the toast component', err);
      loading = null;
      if (first.kind === 'error') window.alert(first.text);
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
export function toast(text: string, kind: Toast['kind'] = 'info', ms = kind === 'error' ? 8000 : 4000): number {
  const id = ++next;
  const t: Toast = { id, kind, text };
  toasts.list.push(t);
  if (!toaster.component) loadToaster(t);
  if (toasts.list.length > MAX) toasts.list.splice(0, toasts.list.length - MAX);
  if (ms > 0) setTimeout(() => dismissToast(id), ms);
  return id;
}

/** Shows an error's message (or a fallback for non-Error values). */
export function toastError(err: unknown) {
  const msg = err instanceof Error ? err.message : typeof err === 'string' ? err : '';
  return toast(msg || 'Something went wrong.', 'error');
}
