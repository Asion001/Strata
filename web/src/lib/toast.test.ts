// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import Toaster from './components/ui/Toaster.svelte';
import { toasts, toast, runToastAction } from './toast.svelte';

// (jsdom has no Web Animations: transitions finish at once.)
Element.prototype.animate ??= function () {
  const a = { onfinish: null as null | (() => void), cancel() {}, play() {}, pause() {}, currentTime: 0, finished: Promise.resolve() };
  queueMicrotask(() => a.onfinish?.());
  return a as unknown as Animation;
};

describe('toast actions', () => {
  it('runs once, closes the toast, and lasts as long as asked', () => {
    vi.useFakeTimers();
    const run = vi.fn();
    const id = toast('“a.txt” moved to Deleted files.', 'success', 10_000, { label: 'Undo', run });
    vi.advanceTimersByTime(9_000);
    expect(toasts.list.some((t) => t.id === id)).toBe(true);
    runToastAction(id);
    runToastAction(id);
    expect(run).toHaveBeenCalledTimes(1);
    expect(toasts.list.some((t) => t.id === id)).toBe(false);
    // (A toast without one: nothing to run, and it stays.)
    const plain = toast('hi');
    runToastAction(plain);
    expect(toasts.list.some((t) => t.id === plain)).toBe(true);
    vi.useRealTimers();
    toasts.list.splice(0);
  });

  it('shows the action as a button that is read with the message', async () => {
    const target = document.body.appendChild(document.createElement('div'));
    const c = mount(Toaster, { target });
    flushSync();
    const run = vi.fn();
    toast('2 items moved to Deleted files.', 'success', 0, { label: 'Undo', run });
    flushSync();
    const status = target.querySelector('[role=status]')!;
    expect(status.textContent).toContain('2 items moved to Deleted files.');
    const btn = [...status.querySelectorAll('button')].find((b) => b.textContent === 'Undo')!;
    expect(btn).toBeTruthy();
    btn.click();
    flushSync();
    expect(run).toHaveBeenCalledTimes(1);
    expect(toasts.list).toHaveLength(0);
    unmount(c);
    target.remove();
  });
});
