import { describe, expect, it } from 'vitest';
import { activeMount, fileHref, folderHref, inMount, isActive, sidebarMounts } from './nav';

describe('isActive', () => {
  it('marks All files only on the root browser', () => {
    expect(isActive('/', '/')).toBe(true);
    expect(isActive('/', '/shares')).toBe(false);
  });
  it('matches a section and its subpages, not lookalike paths', () => {
    expect(isActive('/mounts', '/mounts')).toBe(true);
    expect(isActive('/mounts', '/mounts/3')).toBe(true);
    expect(isActive('/admin', '/administrator')).toBe(false);
  });
});

describe('mounts', () => {
  const list = [{ path: 'Photos' }, { path: 'Photos/Archive' }, { path: 'NAS' }];
  it('finds the deepest mount that holds a folder', () => {
    expect(activeMount(list, 'Photos/Archive/2024')?.path).toBe('Photos/Archive');
    expect(activeMount(list, 'Photos/2026')?.path).toBe('Photos');
    expect(activeMount(list, 'Photosynthesis')).toBeUndefined();
    expect(activeMount(list, '')).toBeUndefined();
  });
  it('treats a mount as containing itself only', () => {
    expect(inMount('NAS', 'NAS')).toBe(true);
    expect(inMount('NAS', 'NAS2')).toBe(false);
  });
  it('encodes folder links', () => {
    expect(folderHref('My Files/a&b')).toBe('/?p=My%20Files%2Fa%26b');
  });
});

describe('sidebarMounts', () => {
  it('leaves out the root mount and labels by last folder', () => {
    const r = sidebarMounts([{ path: '' }, { path: 'Photos' }, { path: 'Work/NAS' }]);
    expect(r.map((m) => m.label)).toEqual(['Photos', 'NAS']);
  });
  it('shows the full path when two mounts end in the same name', () => {
    const r = sidebarMounts([{ path: 'Photos/NAS' }, { path: 'Backup/NAS' }, { path: 'Music' }]);
    expect(r.map((m) => m.label)).toEqual(['Photos/NAS', 'Backup/NAS', 'Music']);
  });
});

describe('fileHref', () => {
  it('links to the folder with the file named', () => {
    expect(fileHref('a/b/c d.txt')).toBe('/?p=a%2Fb&f=c%20d.txt');
    expect(fileHref('top.md')).toBe('/?p=&f=top.md');
  });
});
