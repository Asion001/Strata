// Bulk actions on the files page: delete (to the trash, in batches), move
// and copy of a selection, and Undo. Loaded on first use.
import { api, ApiError, type Entry, type Schemas } from '$lib/api';
import { join } from '$lib/format';
import { reloadMounts } from '$lib/mounts.svelte';
import { confirmDialog, promptDialog } from '$lib/dialogs.svelte';
import { toast, type ToastAction } from '$lib/toast.svelte';
import { trashInfo } from '$lib/trash-info.svelte';
import { names, type FilesController } from './files.svelte';

type Outcome = { done: Entry[]; failed: string[]; skipped: string[]; lastError: string };
const outcome = (): Outcome => ({ done: [], failed: [], skipped: [], lastError: '' });
const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Says what failed (or what was done), refreshes, and leaves what failed
 *  or was kept selected. Returns those names. */
async function finish(c: FilesController, o: Outcome, verb: string, dir: string, fromSelection: boolean, action?: ToastAction, doneText?: string) {
  // (An action, such as Undo, goes on whichever toast shows.)
  const act = o.done.length ? action : undefined;
  const ms = act ? 10_000 : undefined;
  const { done, failed, lastError } = o;
  if (failed.length === 1) toast(`Couldn't ${verb} “${failed[0]}”: ${lastError}`, 'error', ms, act);
  else if (failed.length) toast(`Couldn't ${verb} ${failed.length} items (${failed.slice(0, 3).join(', ')}${failed.length > 3 ? ', …' : ''}). ${lastError}`, 'error', ms, act);
  else if (done.length) toast(doneText ?? `${names(done)} ${verb === 'delete' ? 'deleted' : verb === 'move' ? 'moved' : 'copied'}.`, 'success', ms, act);
  await c.refresh();
  // What failed stays selected for a retry, if the user is still here.
  // (A row-menu action leaves the selection alone.)
  if (fromSelection && c.listedPath === dir) {
    c.selection.clear();
    for (const n of [...failed, ...o.skipped]) if (c.byName.has(n)) c.selection.toggle(n);
  }
  // What wasn't done, failed or kept.
  return [...failed, ...o.skipped];
}

/** Runs `fn` for each item in turn; one failure doesn't stop the rest. */
async function each(c: FilesController, items: Entry[], verb: string, fn: (e: Entry) => Promise<unknown>) {
  const dir = c.listedPath;
  const o = outcome();
  c.busy = true;
  try {
    for (const e of items) {
      try {
        await fn(e);
        o.done.push(e);
      } catch (err) {
        o.failed.push(e.name);
        o.lastError = message(err);
      }
    }
  } finally {
    c.busy = false;
  }
  return finish(c, o, verb, dir, true);
}

/** Mounts are moved and removed on the Mounts page, never as files. */
function withoutMounts(items: Entry[], verb: string) {
  const kept = items.filter((e) => !e.mount_id);
  const skipped = items.length - kept.length;
  if (skipped) toast(`${skipped === 1 ? 'A mount is' : `${skipped} mounts are`} skipped: ${verb} ${skipped === 1 ? 'it' : 'them'} on the Mounts page.`, 'info', 6000);
  return kept;
}

/** Paths per delete request (each one answers per path). */
export const BATCH = 50;
type Result = Schemas['DeleteResult'];

/** Delete `paths`, going on past failures: one result per path. */
async function deleteBatch(paths: string[], permanent: boolean): Promise<Result[]> {
  try {
    return (await api<Schemas['DeletedBody']>('POST', '/api/files/delete', { paths, permanent, keep_going: true })).results;
  } catch (e) {
    // Refused as a whole, or no answer (a gateway timed out, the connection
    // dropped). Not tried again: the server may still be deleting them, and
    // the refreshed listing shows what is gone.
    const refused = e instanceof ApiError && e.status < 500;
    const error = refused ? message(e) : `${message(e)}. Some may have been deleted anyway (to Deleted files, where there is a trash)`;
    return paths.map((p) => ({ path: p, trashed: false, trash_id: null, error, status: e instanceof ApiError ? e.status : null }));
  }
}

/** The names that failed; null when nothing was tried (busy, cancelled). */
export async function removeItems(c: FilesController, all: Entry[], fromSelection = true, permanent = false): Promise<string[] | null> {
  if (c.busy) {
    c.waitForBusy();
    return null;
  }
  const items = withoutMounts(all, 'remove');
  if (!items.length) return null;
  const dir = c.listedPath;
  // Where the server keeps a trash for this folder, deletes go there.
  const toTrash = !permanent && !!c.mount?.trash;
  const one = items.length === 1;
  const listed = `${items.slice(0, 5).map((e) => e.name).join(', ')}${items.length > 5 ? `, and ${items.length - 5} more` : ''}.`;
  const days = ` You can restore ${one ? 'it' : 'them'} there ${trashInfo.days ? `for ${trashInfo.days} days` : 'for a while'}.`;
  const ok = await confirmDialog(
    toTrash
      ? {
          title: `Delete ${names(items)}?`,
          body: (one ? (items[0].is_dir ? 'The folder and everything in it move to Deleted files.' : 'The file moves to Deleted files.') : `${listed} They move to Deleted files, folders with everything in them.`) + days,
          confirm: 'Delete',
          danger: true
        }
      : {
          title: permanent ? `Delete ${names(items)} permanently?` : `Delete ${names(items)}?`,
          body: (one ? (items[0].is_dir ? 'The folder and everything in it will be deleted.' : 'The file will be deleted.') : `${listed} Folders are deleted with everything in them.`) + ' This can’t be undone.',
          confirm: permanent ? 'Delete permanently' : 'Delete',
          danger: true
        }
  );
  if (!ok) return null;
  if (c.busy) {
    c.waitForBusy();
    return null;
  }

  const o = outcome();
  // What went to the trash (with ids, for Undo).
  const trashed: { id: number; path: string }[] = [];
  let inTrash = 0;
  c.busy = true;
  try {
    for (let i = 0; i < items.length; i += BATCH) {
      const batch = items.slice(i, i + BATCH);
      const results = await deleteBatch(batch.map((e) => join(dir, e.name)), permanent);
      for (const [k, e] of batch.entries()) {
        const r = results[k];
        if (!r.error) {
          o.done.push(e);
          if (r.trashed) inTrash++;
          if (r.trash_id != null) trashed.push({ id: r.trash_id, path: r.path });
        } else if (!permanent && r.status === 409 && r.error.includes('delete it permanently')) {
          // The trash can't take it (another disk): for good, if confirmed.
          const sure = await confirmDialog({
            title: `Delete “${e.name}” for good?`,
            body: 'It can’t be moved to Deleted files here, so it will be deleted permanently.',
            confirm: 'Delete permanently',
            danger: true
          });
          if (!sure) {
            o.skipped.push(e.name);
            continue;
          }
          try {
            await api('POST', '/api/files/delete', { paths: [r.path], permanent: true });
            o.done.push(e);
          } catch (err) {
            o.failed.push(e.name);
            o.lastError = message(err);
          }
        } else {
          o.failed.push(e.name);
          o.lastError = r.error;
        }
      }
    }
  } finally {
    c.busy = false;
  }
  const n = o.done.length;
  // (Some may have been deleted for good: the trash couldn't take them.)
  const text = !inTrash ? `${names(o.done)} deleted.` : inTrash === n ? `${names(o.done)} moved to Deleted files.` : `${names(o.done)} deleted: ${inTrash} moved to Deleted files, ${n - inTrash} permanently.`;
  const undoIt = trashed.length ? { label: 'Undo', run: () => void undo(c, trashed) } : undefined;
  return finish(c, o, 'delete', dir, fromSelection, undoIt, text);
}

/** Undo for a delete: brings back exactly what it moved to the trash.
 *  (After whatever runs now: the toast is gone, the Undo mustn't be.) */
export async function undo(c: FilesController, items: { id: number; path: string }[]) {
  while (c.busy) await new Promise((r) => setTimeout(r, 200));
  c.busy = true;
  try {
    // (The restore code loads when Undo is pressed.)
    await (await import('$lib/trash.svelte')).undo(items);
  } catch (e) {
    toast(`Couldn't undo: ${message(e)}. ${items.length === 1 ? 'It is' : 'They are'} still in Deleted files.`, 'error');
  } finally {
    c.busy = false;
  }
  await c.load();
  reloadMounts();
}

export async function transferPicked(c: FilesController, copy: boolean) {
  if (c.busy) return c.waitForBusy();
  const items = copy ? c.picked : withoutMounts(c.picked, 'move');
  if (!items.length) return;
  const dir = c.listedPath;
  const dest = await promptDialog({
    title: `${copy ? 'Copy' : 'Move'} ${names(items)}`,
    label: 'To folder (full path, empty for Home)',
    value: dir,
    required: false,
    confirm: copy ? 'Copy' : 'Move'
  });
  if (dest === null) return;
  // Outer spaces and slashes are never meant: ' docs/ ' is docs.
  const to = dest.trim().replace(/^\/+|\/+$/g, '');
  // Same folder: a move does nothing and a copy would collide with itself.
  if (to === dir) return void toast('Pick a different folder.', 'info');
  if (c.busy) return c.waitForBusy();
  await each(c, items, copy ? 'copy' : 'move', (e) => api('POST', copy ? '/api/files/copy' : '/api/files/move', { from: join(dir, e.name), to: join(to, e.name) }));
}
