// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { renderMarkdown } from './markdown';

describe('markdown preview is inert', () => {
  it.each([
    ['raw script', '<script>alert(1)</script>'],
    ['event handler', '<img src=x onerror="alert(1)">'],
    ['javascript link', '[click](javascript:alert(1))'],
    ['encoded javascript link', '[click](JaVaScRiPt&#58;alert(1))'],
    ['data html link', '[x](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)'],
    ['svg', '<svg onload="alert(1)"><script>alert(1)</script></svg>'],
    ['autolink js', '<javascript:alert(1)>'],
    ['image js', '![x](javascript:alert(1))']
  ])('%s', (_, src) => {
    // Inspect what a browser would build, not the text: escaped "<script>"
    // shown as text is harmless.
    const doc = new DOMParser().parseFromString(`<div>${renderMarkdown(src)}</div>`, 'text/html');
    const all = Array.from(doc.body.querySelectorAll('*'));
    expect(doc.querySelectorAll('script, svg, math, iframe, object, embed, style').length).toBe(0);
    for (const el of all) {
      for (const a of Array.from(el.attributes)) {
        expect(a.name.startsWith('on'), `${el.tagName} ${a.name}`).toBe(false);
        if (['href', 'src', 'xlink:href', 'action', 'formaction'].includes(a.name)) {
          expect(a.value.trim().toLowerCase()).not.toMatch(/^(javascript|data|vbscript):/);
        }
      }
    }
  });

  it('still renders normal markdown', () => {
    const html = renderMarkdown('# Title\n\n**bold** and [link](https://example.com)\n\n```\ncode\n```');
    expect(html).toContain('<h1>Title</h1>');
    expect(html).toContain('<strong>bold</strong>');
    expect(html).toContain('href="https://example.com"');
    expect(html).toContain('rel="noopener noreferrer nofollow"');
    expect(html).toContain('<code>code\n</code>');
  });

  it('shows raw HTML as text', () => {
    expect(renderMarkdown('<b>hi</b>')).toContain('&lt;b&gt;hi&lt;/b&gt;');
  });
});
