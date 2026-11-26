// Whether the server keeps a trash (kept apart from $lib/trash, which the
// startup bundle doesn't need).
import { api, type Schemas } from './api';

/** Days a deleted item stays (0: no trash; null: not known yet), and
 *  whether anything is in the trash (it stays restorable after the trash
 *  is turned off, until its date). */
export const trashInfo = $state<{ days: number | null; any: boolean }>({ days: null, any: false });

/** Whether "Deleted files" belongs in the navigation: unless the server is
 *  known to keep no trash and nothing is left in it. */
export const trashShown = () => trashInfo.days !== 0 || trashInfo.any;

/** Asks the server whether it keeps a trash (and for how long). */
export async function loadTrashInfo(): Promise<void> {
  try {
    const r = await api<Schemas['TrashListBody']>('GET', '/api/trash?limit=1');
    trashInfo.days = r.retention_days;
    trashInfo.any = r.items.length > 0;
  } catch {
    // (Offline or failing: the last answer stands.)
  }
}
