import { describe, expect, it } from 'vitest';
import { cn } from './utils';

describe('cn', () => {
  it('joins truthy class names', () => {
    expect(cn('a', false, null, undefined, 'b', { c: true, d: false })).toBe('a b c');
  });

  // Strata's own color names must be read as colors, not font sizes, so a
  // text color and a text size both survive and a later color replaces one.
  it('keeps text size and Strata color names apart', () => {
    expect(cn('text-sm text-muted-foreground')).toBe('text-sm text-muted-foreground');
    expect(cn('text-sm text-muted-foreground', 'text-success')).toBe('text-sm text-success');
    expect(cn('bg-sidebar-accent', 'bg-destructive')).toBe('bg-destructive');
  });
});
