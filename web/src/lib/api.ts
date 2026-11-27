// Thin client for the Strata HTTP API.
export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
    /** The error's kind, e.g. "exists" on a 409 for a taken name. */
    public code?: string | null,
    /** With "exists": what is there. */
    public details?: Existing | null
  ) {
    super(message);
  }
}

import { untrack } from 'svelte';
import { session } from './session.svelte';

import { OFFLINE_AT } from './offline/keys';

export async function api<T = any>(method: string, url: string, body?: unknown, meta?: { offlineAt?: string | null }): Promise<T> {
  const init: RequestInit = { method, headers: { 'x-strata': '1' }, credentials: 'same-origin' };
  // The service worker keeps listings per user: it needs to know whose.
  // (Read untracked: a page's loading effect mustn't re-run when it changes.)
  const user = untrack(() => session.user?.id);
  if (method === 'GET' && user) (init.headers as Record<string, string>)['x-strata-user'] = String(user);
  if (body !== undefined) {
    (init.headers as Record<string, string>)['content-type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  const r = await fetch(url, init);
  if (meta) meta.offlineAt = r.headers.get(OFFLINE_AT);
  if (!r.ok) {
    let msg = r.statusText;
    let code: string | null = null;
    let details: Existing | null = null;
    try {
      const b = await r.json();
      msg = b.error ?? msg;
      code = b.code ?? null;
      details = b.details ?? null;
    } catch {}
    if (r.status === 401 && !url.startsWith('/api/public/')) forgetOfflineUser();
    if (r.status === 401 && !url.startsWith('/api/auth/') && !url.startsWith('/api/public/')) {
      location.href = '/login?next=' + encodeURIComponent(location.pathname + location.search);
    }
    throw new ApiError(r.status, msg, code, details);
  }
  return r.json();
}

import { isSvg } from './format';

export const enc = encodeURIComponent;

// The API's types come from its OpenAPI document (api/openapi.json, made
// from the server's code): `npm run api:types` writes ./openapi.d.ts.
import type { components, paths } from './openapi';
export type Schemas = components['schemas'];
export type Entry = Schemas['Entry'];
export type User = Schemas['User'];
/** What is in the way of a write, move or copy (a 409 "exists"). */
export type Existing = Schemas['Existing'];

type Method = 'get' | 'put' | 'post' | 'delete';
/** The methods a path has (not `parameters`, nor ones it lacks). */
export type MethodOf<P extends keyof paths> = {
  [M in Method]: paths[P][M] extends never | undefined ? never : M;
}[Method];
type Op<P extends keyof paths, M extends MethodOf<P>> = paths[P][M];
type JsonOf<R> = R extends { content: { 'application/json': infer T } } ? T : never;
/** The JSON answer of an operation (`ApiOk<'/api/auth/me', 'get'>`). */
export type ApiOk<P extends keyof paths, M extends MethodOf<P>> = Op<P, M> extends { responses: { 200: infer R } } ? JsonOf<R> : never;
/** The JSON body an operation takes (undefined: none). */
export type ApiBody<P extends keyof paths, M extends MethodOf<P>> = Op<P, M> extends {
  requestBody: { content: { 'application/json': infer B } };
}
  ? B
  : undefined;
/** The values of a path's `{placeholders}`. */
export type ApiPathParams<P extends keyof paths, M extends MethodOf<P>> = Op<P, M> extends {
  parameters: { path: infer X };
}
  ? X
  : never;

/** `api()`, typed by the OpenAPI document: path (its placeholders filled
 *  in from `params`), method, body and answer. */
export function call<P extends keyof paths, M extends MethodOf<P>>(
  method: M,
  path: P,
  opts: { params?: ApiPathParams<P, M>; body?: ApiBody<P, M>; query?: string } = {}
): Promise<ApiOk<P, M>> {
  const params = (opts.params ?? {}) as Record<string, string | number>;
  const url = String(path).replace(/\{(\w+)\}/g, (_, k: string) => {
    if (params[k] === undefined) throw new Error(`call: no value for {${k}} in ${String(path)}`);
    return enc(String(params[k]));
  });
  return api(method.toUpperCase(), url + (opts.query ?? ''), opts.body);
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

/** The session is gone (401): this device no longer opens offline as that
 *  user (their cached data stays until a sign-out or another sign-in). */
export function forgetOfflineUser() {
  try {
    localStorage.removeItem('strata-last-user');
  } catch {}
  if (typeof caches !== 'undefined') void caches.delete('strata-meta').catch(() => false);
}
