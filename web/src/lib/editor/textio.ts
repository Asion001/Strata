// Text handling for the editor: UTF-8 only, line endings and the trailing
// newline preserved exactly.

/** A file's line ending. 'keep' is for files with mixed endings or lone
 *  CRs: their text goes to the editor unchanged (CR shown as a character),
 *  so every byte is written back exactly as it was. */
export type Eol = '\n' | '\r\n' | 'keep';

export const MAX_EDIT_BYTES = 5 * 1024 * 1024;
/** Above this, the file isn't loaded into the browser at all. */
export const MAX_VIEW_BYTES = 20 * 1024 * 1024;
const NUL_SCAN = 8 * 1024;

/** True if the first 8 KB contain a NUL byte (a binary file). */
export function looksBinary(bytes: Uint8Array): boolean {
  return bytes.subarray(0, NUL_SCAN).includes(0);
}

/** Strict UTF-8 decode; null if the bytes are not valid UTF-8. */
export function decodeUtf8(bytes: Uint8Array): string | null {
  try {
    return new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes);
  } catch {
    return null;
  }
}

/** The file's line ending: LF, CRLF, or 'keep' when they are mixed. */
export function detectEol(text: string): Eol {
  let crlf = 0;
  let lf = 0;
  for (let i = text.indexOf('\n'); i !== -1; i = text.indexOf('\n', i + 1)) {
    if (i > 0 && text[i - 1] === '\r') crlf++;
    else lf++;
  }
  const loneCr = /\r(?!\n)/.test(text);
  if (loneCr || (crlf > 0 && lf > 0)) return 'keep';
  return crlf > 0 ? '\r\n' : '\n';
}

/** File text -> editor document. The editor only splits lines on LF, so a
 *  CRLF file is shown with LF and written back with CRLF; anything mixed is
 *  shown and written back verbatim. */
export function toEditor(text: string): { doc: string; eol: Eol } {
  const eol = detectEol(text);
  return { doc: eol === '\r\n' ? text.replace(/\r\n/g, '\n') : text, eol };
}

/** Editor document -> file text, restoring the file's line ending. Whether
 *  the file ends with a newline is kept as-is: the editor never adds or
 *  removes one. */
export function fromEditor(doc: string, eol: Eol): string {
  return eol === '\r\n' ? doc.replace(/\r?\n/g, '\r\n') : doc;
}

/** "notes (conflict 2026-11-17 14-05).md" for "notes.md". */
export function conflictName(name: string, now = new Date(), seconds = false): string {
  const p = (n: number) => String(n).padStart(2, '0');
  const stamp =
    `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())} ${p(now.getHours())}-${p(now.getMinutes())}` +
    (seconds ? `-${p(now.getSeconds())}` : '');
  const dot = name.lastIndexOf('.');
  return dot > 0
    ? `${name.slice(0, dot)} (conflict ${stamp})${name.slice(dot)}`
    : `${name} (conflict ${stamp})`;
}
