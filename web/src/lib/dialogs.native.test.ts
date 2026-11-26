// When the dialog component can't load (offline after an update), the
// browser's own dialogs stand in, so nothing waits forever.
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('$lib/components/ui/DialogHost.svelte', () => {
  throw new Error('offline');
});

const { pickFolder } = await import('./dialogs.svelte');

const stub = (answer: string | null) => {
  const prompt = vi.fn(() => answer);
  const alert = vi.fn();
  vi.stubGlobal('window', { prompt, alert, confirm: vi.fn(() => true) });
  return { prompt, alert };
};
afterEach(() => vi.unstubAllGlobals());
vi.spyOn(console, 'error').mockImplementation(() => {});

describe('folder pick without the dialog component', () => {
  it('asks for a path, starting at the current folder', async () => {
    const { prompt } = stub(' /b/c/ ');
    expect(await pickFolder({ title: 'Move', start: 'a', from: 'a' })).toBe('b/c');
    expect(prompt).toHaveBeenCalledWith(expect.stringContaining('Move'), 'a');
  });

  it('refuses the folder the items are in', async () => {
    const { alert } = stub('/a/');
    expect(await pickFolder({ title: 'Move', start: 'a', from: 'a' })).toBeNull();
    expect(alert).toHaveBeenCalledWith('Pick a different folder.');
  });

  it('answers a cancel with null', async () => {
    stub(null);
    expect(await pickFolder({ title: 'Move', start: 'a' })).toBeNull();
  });
});
