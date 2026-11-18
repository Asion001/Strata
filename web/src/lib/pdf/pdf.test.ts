import { describe, expect, it } from 'vitest';
import { safeExternalUrl } from './links';

describe('PDF links', () => {
  it('allows only http and https', () => {
    expect(safeExternalUrl('https://example.org/a?b#c')).toBe('https://example.org/a?b#c');
    expect(safeExternalUrl('http://example.org')).toBe('http://example.org/');
    for (const bad of ['javascript:alert(1)', 'JaVaScRiPt:alert(1)', 'data:text/html,x', 'file:///etc/passwd', 'vbscript:x', 'mailto:a@b.c', 'relative/path', '']) {
      expect(safeExternalUrl(bad)).toBeNull();
    }
  });
});
