import { describe, expect, it } from 'vitest';
import { conflictName, decodeUtf8, detectEol, fromEditor, looksBinary, toEditor } from './textio';

const roundtrip = (t: string) => {
  const { doc, eol } = toEditor(t);
  return fromEditor(doc, eol);
};

describe('line endings and trailing newline', () => {
  it.each([
    ['lf with trailing newline', 'a\nb\n'],
    ['lf without trailing newline', 'a\nb'],
    ['crlf with trailing newline', 'a\r\nb\r\n'],
    ['crlf without trailing newline', 'a\r\nb'],
    ['empty', ''],
    ['only newline', '\n'],
    ['only crlf', '\r\n'],
    ['no newline at all', 'single line'],
    ['blank lines at end', 'a\r\n\r\n\r\n']
  ])('round-trips %s unchanged', (_, text) => {
    expect(roundtrip(text)).toBe(text);
  });

  it('edits keep the file line ending', () => {
    const { doc, eol } = toEditor('one\r\ntwo\r\n');
    expect(doc).toBe('one\ntwo\n');
    expect(fromEditor(doc + 'three\n', eol)).toBe('one\r\ntwo\r\nthree\r\n');
  });

  it('mixed files are kept byte for byte', () => {
    expect(detectEol('a\r\nb\r\nc\n')).toBe('keep');
    expect(roundtrip('a\r\nb\nc\r\n')).toBe('a\r\nb\nc\r\n');
    const { doc, eol } = toEditor('a\r\nb\n');
    expect(fromEditor(doc + 'c\n', eol)).toBe('a\r\nb\nc\n');
  });

  it('a lone CR stays a CR in LF files', () => {
    expect(roundtrip('a\rb\n')).toBe('a\rb\n');
  });
});

describe('binary and encoding checks', () => {
  it('flags NUL bytes in the first 8 KB only', () => {
    expect(looksBinary(new Uint8Array([104, 0, 105]))).toBe(true);
    const late = new Uint8Array(9000).fill(65);
    late[8500] = 0;
    expect(looksBinary(late)).toBe(false);
  });
  it('rejects invalid UTF-8', () => {
    expect(decodeUtf8(new Uint8Array([0xcf, 0xf0]))).toBeNull();
    expect(decodeUtf8(new TextEncoder().encode('Привіт'))).toBe('Привіт');
  });
  it('keeps a BOM so it is written back', () => {
    expect(decodeUtf8(new Uint8Array([0xef, 0xbb, 0xbf, 0x61]))).toBe('﻿a');
  });
});

describe('conflict names', () => {
  const d = new Date(2026, 10, 17, 14, 5, 9);
  it('inserts the stamp before the extension', () => {
    expect(conflictName('notes.md', d)).toBe('notes (conflict 2026-11-17 14-05).md');
    expect(conflictName('Makefile', d)).toBe('Makefile (conflict 2026-11-17 14-05)');
    expect(conflictName('.env', d)).toBe('.env (conflict 2026-11-17 14-05)');
    expect(conflictName('a.tar.gz', d, true)).toBe('a.tar (conflict 2026-11-17 14-05-09).gz');
  });
});
