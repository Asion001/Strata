/** A lazily loaded part of the app failed to load. Usually the app was
 *  updated since this page opened, so its old chunks are gone: reload once
 *  to pick up the new build (never in a loop). */
export function staleApp(err: unknown) {
  console.error('could not load part of the app', err);
  const key = 'strata-reloaded-for-chunk';
  try {
    if (sessionStorage.getItem(key)) {
      alert('Part of the app could not load. Check the connection and try again.');
      return;
    }
    sessionStorage.setItem(key, '1');
    setTimeout(() => sessionStorage.removeItem(key), 60_000);
  } catch {}
  location.reload();
}
