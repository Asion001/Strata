// Actions on one entry of the files page (and New folder). Loaded on first
// use. Each action fixes its target before asking: the folder may change
// (Back, a sidebar click) while the dialog is open.
import { api, contentUrl, type Entry } from '$lib/api';
import { join } from '$lib/format';
import { userRecorded } from '$lib/offline/app.svelte';
import { session } from '$lib/session.svelte';
import { alertDialog, formDialog, pickFolder, promptDialog } from '$lib/dialogs.svelte';
import { toast, toastError } from '$lib/toast.svelte';
import type { FilesController } from './files.svelte';

export async function newFolder(c: FilesController) {
  const dir = c.path;
  const name = await promptDialog({ title: 'New folder', label: 'Name', confirm: 'Create' });
  if (name) void c.act(() => api('POST', '/api/files/mkdir', { path: join(dir, name) }));
}

export async function rename(c: FilesController, e: Entry) {
  c.menu = null;
  const dir = c.listedPath;
  const name = await promptDialog({ title: `Rename “${e.name}”`, label: 'New name', value: e.name, confirm: 'Rename' });
  if (name && name !== e.name) void c.act(() => api('POST', '/api/files/move', { from: join(dir, e.name), to: join(dir, name) }));
}

export async function moveTo(c: FilesController, e: Entry, copy = false) {
  c.menu = null;
  const dir = c.listedPath;
  const from = join(dir, e.name);
  const dest = await pickFolder({
    title: `${copy ? 'Copy' : 'Move'} “${e.name}”`,
    confirm: copy ? 'Copy here' : 'Move here',
    start: dir,
    from: dir,
    // A folder can't go inside itself (a copy neither).
    exclude: e.is_dir ? [from] : []
  });
  if (dest !== null) void c.act(() => api('POST', copy ? '/api/files/copy' : '/api/files/move', { from, to: join(dest, e.name) }));
}

export async function share(c: FilesController, e: Entry) {
  c.menu = null;
  const target = join(c.listedPath, e.name);
  const f = await formDialog({
    title: `Share “${e.name}”`,
    body: 'Anyone with the link can open it.',
    confirm: 'Create link',
    fields: [
      { name: 'password', label: 'Password', type: 'password', hint: 'Optional. Leave empty for none.' },
      { name: 'days', label: 'Expires after (days)', type: 'number', min: 1, hint: 'Optional. Leave empty to never expire.' }
    ]
  });
  if (!f) return;
  try {
    const s = await api('POST', '/api/shares', {
      path: target,
      password: f.password || null,
      // The field allows only whole days from 1 up.
      expires_at: f.days ? new Date(Date.now() + Number(f.days) * 86400000).toISOString() : null
    });
    const url = `${location.origin}/s/${s.token}`;
    let copied = false;
    try {
      await navigator.clipboard.writeText(url);
      copied = true;
    } catch {}
    await alertDialog({ title: 'Share link', body: copied ? 'Copied to the clipboard.' : undefined, copy: url, ok: 'Done' });
    return s;
  } catch (err) {
    toastError(err);
  }
}

export async function togglePin(c: FilesController, e: Entry) {
  c.menu = null;
  const p = join(c.listedPath, e.name);
  const on = !c.pinned.has(p);
  // (Right after the page loads, who is signed in may not be recorded yet.)
  await userRecorded();
  if (!session.user) return;
  const r = await (await import('$lib/offline/recent')).pin(session.user.id, p, e.etag, contentUrl(p), on, e.size).catch(() => 'failed' as const);
  const why = { 'no-version': 'it has no version to keep', unreadable: "it couldn't be read", 'too-big': 'it is too big, or offline storage is full', failed: 'offline storage is not available' };
  if (r !== true) toast(`Couldn't keep “${e.name}” offline: ${why[r]}.`, 'error');
  else toast(on ? `“${e.name}” is kept offline.` : `“${e.name}” is no longer kept offline.`, 'success');
  await c.loadPins();
}
