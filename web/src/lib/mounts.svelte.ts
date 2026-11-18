import { api } from './api';

export type MountInfo = {
  id: number;
  path: string;
  mode: string;
  connection_id: number;
  connection_name: string;
  connection_kind: string;
  base_path: string;
};

/** The signed-in user's mounts, shared by the sidebar and the Mounts page. */
export const mounts = $state<{ list: MountInfo[]; loaded: boolean }>({ list: [], loaded: false });

let seq = 0;

/** Fetches the mounts again. An older request that finishes late is ignored. */
export async function reloadMounts(): Promise<void> {
  const my = ++seq;
  try {
    const list = await api<MountInfo[]>('GET', '/api/mounts');
    if (my === seq) mounts.list = list;
  } catch {
    // The sidebar just keeps the last list; the Mounts page shows its own errors.
  } finally {
    if (my === seq) mounts.loaded = true;
  }
}
