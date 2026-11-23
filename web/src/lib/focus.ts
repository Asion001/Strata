// Keeps keyboard focus inside a modal overlay: Tab and Shift+Tab wrap around
// its own controls, focus moves into it when it opens and goes back to where
// it was when it closes.

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"]), video[controls], audio[controls]';

export function focusables(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    (el) => !el.closest('[inert], [hidden]') && getComputedStyle(el).visibility !== 'hidden'
  );
}

/** Svelte action: trap focus inside `node` while it is mounted. */
export function trapFocus(node: HTMLElement) {
  const before = document.activeElement as HTMLElement | null;
  if (!node.contains(document.activeElement)) node.focus({ preventScroll: true });
  const key = (e: KeyboardEvent) => {
    if (e.key !== 'Tab') return;
    const all = focusables(node);
    if (!all.length) {
      e.preventDefault();
      node.focus();
      return;
    }
    const first = all[0], last = all[all.length - 1];
    const at = document.activeElement;
    if (e.shiftKey && (at === first || at === node || !node.contains(at))) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && (at === last || !node.contains(at))) {
      e.preventDefault();
      first.focus();
    }
  };
  node.addEventListener('keydown', key);
  return {
    destroy() {
      node.removeEventListener('keydown', key);
      if (before?.isConnected) before.focus({ preventScroll: true });
    }
  };
}
