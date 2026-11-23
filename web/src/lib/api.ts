// Thin client for the Strata HTTP API.
export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

import { session } from './session.svelte';

import { OFFLINE_AT } from './offline/keys';

export async function api<T = any>(method: string, url: string, body?: unknown, meta?: { offlineAt?: string | null }): Promise<T> {
  const init: RequestInit = { method, headers: { 'x-strata': '1' }, credentials: 'same-origin' };
  // The service worker keeps listings per user: it needs to know whose.
  if (method === 'GET' && session.user) (init.headers as Record<string, string>)['x-strata-user'] = String(session.user.id);
  if (body !== undefined) {
    (init.headers as Record<string, string>)['content-type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  const r = await fetch(url, init);
  if (meta) meta.offlineAt = r.headers.get(OFFLINE_AT);
  if (!r.ok) {
    let msg = r.statusText;
    try {
      msg = (await r.json()).error ?? msg;
    } catch {}
    if (r.status === 401 && !url.startsWith('/api/auth/') && !url.startsWith('/api/public/')) {
      location.href = '/login?next=' + encodeURIComponent(location.pathname + location.search);
    }
    throw new ApiError(r.status, msg);
  }
  return r.json();
}

import { isSvg } from './format';

export const enc = encodeURIComponent;

export interface Entry {
  name: string;
  is_dir: boolean;
  size: number;
  mtime: string;
  etag: string | null;
  mount_id: number | null;
  /** A mount point whose storage is offline. */
  offline?: boolean;
}

export interface User {
  id: number;
  username: string;
  is_admin: boolean;
}

export function contentUrl(path: string, download = false) {
  return `/api/files/content?path=${enc(path)}${download ? '&download=1' : ''}`;
}
/** Image-preview route: the only place SVG is served as an image. Use it
 *  only in <img> elements. */
export function imageUrl(path: string) {
  return `/api/files/image?path=${enc(path)}`;
}
/** Pick the preview source: SVG only ever through the image route (and
 *  then only inside <img>), everything else through the content route. */
export function pickPreview(name: string, content: string, image: string) {
  return isSvg(name) ? image : content;
}
export function previewUrl(path: string) {
  return pickPreview(path, contentUrl(path), imageUrl(path));
}
export function thumbUrl(path: string, etag: string | null, size = 128) {
  return `/api/files/thumb?path=${enc(path)}&size=${size}&v=${enc(etag ?? '')}`;
}
