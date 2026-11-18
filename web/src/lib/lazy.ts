import { version } from '$app/environment';
import { toast } from '$lib/toast.svelte';

/** A lazily loaded part of the app failed to load. If the app was updated
 *  since this page opened (its old chunks are gone), reload to pick up the
 *  new build, at most once a minute; otherwise it was the network, so say so
 *  and keep the page (and any running uploads). */
export async function staleApp(err: unknown) {
  console.error('could not load part of the app', err);
  let updated = false;
  try {
    const r = await fetch('/_app/version.json', { cache: 'no-store' });
    updated = r.ok && (await r.json()).version !== version;
  } catch {}
  const key = 'strata-reloaded-for-update';
  let recent = false;
  try {
    recent = Date.now() - Number(sessionStorage.getItem(key) ?? 0) < 60_000;
    if (updated && !recent) sessionStorage.setItem(key, String(Date.now()));
  } catch {}
  if (updated && !recent) location.reload();
  else toast('Part of the app could not load. Check the connection and try again.', 'error');
}
