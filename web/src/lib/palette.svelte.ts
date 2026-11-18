// The ⌘K command palette: opened from anywhere, loaded on first use to keep
// it out of the startup bundle. The root layout renders it once loaded.
import type { Component } from 'svelte';
import { staleApp } from '$lib/lazy';

export const palette = $state<{ open: boolean; component: Component | null }>({ open: false, component: null });
let loading: Promise<void> | null = null;

export function openPalette() {
  palette.open = true;
  loading ??= import('$lib/components/shell/CommandPalette.svelte').then(
    (m) => void (palette.component = m.default),
    (err) => {
      loading = null;
      palette.open = false;
      staleApp(err);
    }
  );
}

/** Ctrl/Cmd+K anywhere toggles the palette. */
export function paletteKey(e: KeyboardEvent) {
  if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'k') {
    e.preventDefault();
    if (palette.open) palette.open = false;
    else openPalette();
  }
}
