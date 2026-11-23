// Keeps keyboard focus inside a modal overlay: Tab and Shift+Tab wrap around
// its own controls, focus moves into it when it opens and goes back to where
// it was when it closes.

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"]), video[controls], audio[controls]';

export function focusables(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    (el) =>
      !el.closest('[inert], [hidden]') &&
      // Rendered and visible (display: none too, e.g. buttons hidden on touch screens).
      (el.checkVisibility ? el.checkVisibility({ visibilityProperty: true }) : getComputedStyle(el).visibility !== 'hidden')
  );
}

/** Svelte action: trap focus inside `node` while it is mounted. */
export function trapFocus(node: HTMLElement) {
  const before = document.activeElement as HTMLElement | null;
  if (!node.contains(document.activeElement)) node.focus({ preventScroll: true });
  const key = (e: KeyboardEvent) => {
    if (e.key !== 'Tab') return;
    // (A modal dialog over the overlay keeps its own focus.)
    if (document.querySelector('dialog[open]')) return;
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
  // On the document (capturing): focus can fall to <body> when the focused
  // control goes away (Next on the last file), and Tab must still wrap.
  document.addEventListener('keydown', key, true);
  return {
    destroy() {
      document.removeEventListener('keydown', key, true);
      if (before?.isConnected) before.focus({ preventScroll: true });
    }
  };
}
