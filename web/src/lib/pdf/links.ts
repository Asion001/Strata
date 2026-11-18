/** Only plain web links leave the viewer: http(s), in a new tab, with no
 *  opener or referrer. Anything else (javascript:, file:, data:, custom
 *  schemes) is shown disabled. */
export function safeExternalUrl(url: string): string | null {
  try {
    const u = new URL(url);
    return u.protocol === 'http:' || u.protocol === 'https:' ? u.href : null;
  } catch {
    return null;
  }
}
