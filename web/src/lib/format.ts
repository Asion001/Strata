export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const u = ['KB', 'MB', 'GB', 'TB', 'PB'];
  let i = -1;
  do {
    n /= 1024;
    i++;
  } while (n >= 1024 && i < u.length - 1);
  return `${n.toFixed(n < 10 ? 1 : 0)} ${u[i]}`;
}

export function when(iso: string): string {
  const d = new Date(iso);
  const now = new Date();
  if (d.toDateString() === now.toDateString())
    return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  return d.toLocaleDateString(undefined, { year: d.getFullYear() === now.getFullYear() ? undefined : 'numeric', month: 'short', day: 'numeric' });
}

export function join(a: string, b: string) {
  return a ? (b ? `${a}/${b}` : a) : b;
}

export function parent(p: string) {
  const i = p.lastIndexOf('/');
  return i < 0 ? '' : p.slice(0, i);
}

const IMG = /\.(jpe?g|png|gif|webp|bmp|tiff?)$/i;
export const isImage = (n: string) => IMG.test(n);
export const isVideo = (n: string) => /\.(mp4|webm|mov|m4v)$/i.test(n);
export const isAudio = (n: string) => /\.(mp3|m4a|flac|ogg|wav|opus)$/i.test(n);

export function icon(e: { name: string; is_dir: boolean; mount_id?: number | null }): string {
  if (e.mount_id) return '🗄️';
  if (e.is_dir) return '📁';
  if (isImage(e.name)) return '🖼️';
  if (isVideo(e.name)) return '🎞️';
  if (isAudio(e.name)) return '🎵';
  if (/\.(zip|tar|gz|7z|rar|zst)$/i.test(e.name)) return '🗜️';
  if (/\.(pdf)$/i.test(e.name)) return '📕';
  return '📄';
}
