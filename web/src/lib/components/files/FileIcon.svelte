<script lang="ts" module>
  import { kindOf, type Kind } from '$lib/format';
  import { cn } from '$lib/utils';
  import type { Component } from 'svelte';
  import FolderIcon from '@lucide/svelte/icons/folder';
  import HardDriveIcon from '@lucide/svelte/icons/hard-drive';
  import FileImageIcon from '@lucide/svelte/icons/file-image';
  import FilePlayIcon from '@lucide/svelte/icons/file-play';
  import FileMusicIcon from '@lucide/svelte/icons/file-music';
  import FileArchiveIcon from '@lucide/svelte/icons/file-archive';
  import FileTextIcon from '@lucide/svelte/icons/file-text';
  import FileIcon from '@lucide/svelte/icons/file';


  // Built once for every icon on the page.
  const KINDS: Record<Kind, [Component<any>, string]> = {
    mount: [HardDriveIcon, 'text-primary'],
    dir: [FolderIcon, 'text-primary fill-primary/15'],
    image: [FileImageIcon, 'text-emerald-500'],
    video: [FilePlayIcon, 'text-violet-500'],
    audio: [FileMusicIcon, 'text-pink-500'],
    archive: [FileArchiveIcon, 'text-amber-500'],
    pdf: [FileTextIcon, 'text-red-500'],
    text: [FileTextIcon, 'text-muted-foreground'],
    file: [FileIcon, 'text-muted-foreground']
  };
</script>

<script lang="ts">
  // A Lucide icon for an entry's kind (see kindOf), tinted per kind.
  let { entry, class: className = '' }: { entry: { name: string; is_dir: boolean; mount_id?: number | null }; class?: string } = $props();

  const [Icon, tone] = $derived(KINDS[kindOf(entry)]);
</script>

<Icon class={cn('shrink-0', tone, className)} aria-hidden="true" strokeWidth={1.75} />
