<script lang="ts">
  // A Lucide icon for an entry's kind, tinted per kind.
  import { isAudio, isImage, isTextName, isVideo } from '$lib/format';
  import { cn } from '$lib/utils';
  import FolderIcon from '@lucide/svelte/icons/folder';
  import HardDriveIcon from '@lucide/svelte/icons/hard-drive';
  import FileImageIcon from '@lucide/svelte/icons/file-image';
  import FilePlayIcon from '@lucide/svelte/icons/file-play';
  import FileMusicIcon from '@lucide/svelte/icons/file-music';
  import FileArchiveIcon from '@lucide/svelte/icons/file-archive';
  import FileTextIcon from '@lucide/svelte/icons/file-text';
  import FileIcon from '@lucide/svelte/icons/file';

  let { entry, class: className = '' }: { entry: { name: string; is_dir: boolean; mount_id?: number | null }; class?: string } = $props();

  const kind = $derived.by(() => {
    const n = entry.name;
    if (entry.mount_id) return { icon: HardDriveIcon, tone: 'text-primary' };
    if (entry.is_dir) return { icon: FolderIcon, tone: 'text-primary fill-primary/15' };
    if (isImage(n)) return { icon: FileImageIcon, tone: 'text-emerald-500' };
    if (isVideo(n)) return { icon: FilePlayIcon, tone: 'text-violet-500' };
    if (isAudio(n)) return { icon: FileMusicIcon, tone: 'text-pink-500' };
    if (/\.(zip|tar|gz|7z|rar|zst)$/i.test(n)) return { icon: FileArchiveIcon, tone: 'text-amber-500' };
    if (/\.pdf$/i.test(n)) return { icon: FileTextIcon, tone: 'text-red-500' };
    if (isTextName(n)) return { icon: FileTextIcon, tone: 'text-muted-foreground' };
    return { icon: FileIcon, tone: 'text-muted-foreground' };
  });
</script>

<kind.icon class={cn('shrink-0', kind.tone, className)} aria-hidden="true" strokeWidth={1.75} />
