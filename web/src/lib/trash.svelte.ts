// Deleted files: whether the server keeps a trash, the trash listing, and
// restoring (from the trash page, or Undo after a delete).
import { goto } from '$app/navigation';
import { api, type Schemas } from './api';
import { parent } from './format';
import { folderHref } from './components/shell/nav';
import { toast, type ToastAction } from './toast.svelte';
import { trashInfo } from './trash-info.svelte';

export { trashInfo, loadTrashInfo } from './trash-info.svelte';

export type TrashItem = Schemas['TrashItem'];

/** Every item in the user's trash, newest first (fetched in pages). */
export async function listTrash(): Promise<{ days: number; items: TrashItem[] }> {
  const items: TrashItem[] = [];
  let before: number | undefined;
  for (;;) {
    const page = await api<Schemas['TrashListBody']>('GET', `/api/trash?limit=1000${before ? `&before=${before}` : ''}`);
    trashInfo.days = page.retention_days;
    items.push(...page.items);
    if (!page.more || !page.items.length) {
      trashInfo.any = items.length > 0;
      return { days: page.retention_days, items };
    }
    before = page.items[page.items.length - 1].id;
  }
}

export type Restored = { id: number; from: string; path: string };
export type RestoreResult = { restored: Restored[]; failed: { id: number; from: string; error: string }[] };

/** Restores items one by one; one failure doesn't stop the rest. */
export async function restoreItems(items: { id: number; path: string }[]): Promise<RestoreResult> {
  const out: RestoreResult = { restored: [], failed: [] };
  for (const it of items) {
    try {
      const r = await api<Schemas['PathResultBody']>('POST', `/api/trash/${it.id}/restore`, {});
      out.restored.push({ id: it.id, from: it.path, path: r.path });
    } catch (e) {
      out.failed.push({ id: it.id, from: it.path, error: e instanceof Error ? e.message : String(e) });
    }
  }
  return out;
}

const base = (p: string) => p.slice(p.lastIndexOf('/') + 1);
const openFolder = (path: string): ToastAction => ({ label: 'Open folder', run: () => void goto(folderHref(parent(path))) });

/** Says how a restore went: what failed, what came back under a new name
 *  (with a way to its folder), or that it all came back. */
export function reportRestore(r: RestoreResult) {
  if (r.failed.length === 1) toast(`Couldn't restore “${base(r.failed[0].from)}”: ${r.failed[0].error}`, 'error');
  else if (r.failed.length) toast(`Couldn't restore ${r.failed.length} items. ${r.failed[0].error}`, 'error');
  const renamed = r.restored.filter((x) => x.path !== x.from);
  if (renamed.length === 1) toast(`Restored as “${base(renamed[0].path)}”, since “${base(renamed[0].from)}” is taken.`, 'success', 8000, openFolder(renamed[0].path));
  else if (renamed.length) toast(`${renamed.length} items were restored under new names, since theirs are taken.`, 'success', 8000);
  else if (!r.failed.length && r.restored.length) {
    const one = r.restored.length === 1;
    toast(one ? `“${base(r.restored[0].path)}” restored.` : `${r.restored.length} items restored.`, 'success', 4000, one ? openFolder(r.restored[0].path) : undefined);
  }
}

/** Undo for a delete: restores exactly these trash items (by id, so a
 *  folder opened since doesn't matter), and says how it went. */
export async function undo(items: { id: number; path: string }[]): Promise<void> {
  reportRestore(await restoreItems(items));
}
