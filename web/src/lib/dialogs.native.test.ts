// When the dialog component can't load (offline after an update), the
// browser's own dialogs stand in, so nothing waits forever.
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('$lib/components/ui/DialogHost.svelte', () => {
  throw new Error('offline');
});

const { pickFolder, conflictDialog } = await import('./dialogs.svelte');

const stub = (answer: string | null, yes = true) => {
  const prompt = vi.fn(() => answer);
  const alert = vi.fn();
  const confirm = vi.fn(() => yes);
  vi.stubGlobal('window', { prompt, alert, confirm });
  return { prompt, alert, confirm };
};
afterEach(() => vi.unstubAllGlobals());
vi.spyOn(console, 'error').mockImplementation(() => {});

describe('folder pick without the dialog component', () => {
  it('asks for a path, starting at the current folder', async () => {
    const { prompt } = stub(' /b//c/ ');
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

describe('a name conflict without the dialog component', () => {
  const side = { is_dir: false };
  it('offers only what loses nothing: keep both, or skip', async () => {
    const { confirm } = stub(null, true);
    expect(await conflictDialog({ title: 'Taken', name: 'x', existing: side, incoming: side })).toEqual({ action: 'rename', all: false });
    expect(confirm).toHaveBeenCalledWith(expect.stringContaining('Keep both?'));
    stub(null, false);
    expect(await conflictDialog({ title: 'Taken', name: 'x', existing: side, incoming: side })).toEqual({ action: 'skip', all: false });
  });
});
