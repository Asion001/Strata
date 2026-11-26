// Whether the server keeps a trash (kept apart from $lib/trash, which the
// startup bundle doesn't need).
import { api, type Schemas } from './api';

/** Days a deleted item stays (0: no trash; null: not known yet). */
export const trashInfo = $state<{ days: number | null }>({ days: null });

/** Asks the server whether it keeps a trash (and for how long). */
export async function loadTrashInfo(): Promise<void> {
  try {
    trashInfo.days = (await api<Schemas['TrashListBody']>('GET', '/api/trash?limit=1')).retention_days;
  } catch {
    // (Offline or failing: the last answer stands.)
  }
}
