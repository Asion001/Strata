<script lang="ts">
  // ⌘K: jump to a page, or find a file or folder by name. A native modal
  // <dialog>, like DialogHost, so it sits above everything and traps focus.
  import { goto } from '$app/navigation';
  import { api, type Schemas } from '$lib/api';
  import { bytes } from '$lib/format';
  import { cn } from '$lib/utils';
  import { palette } from '$lib/palette.svelte';
  import { session } from '$lib/session.svelte';
  import FileIcon from '$lib/components/files/FileIcon.svelte';
  import SearchIcon from '@lucide/svelte/icons/search';
  import SettingsIcon from '@lucide/svelte/icons/settings';
  import { fileHref, folderHref, mainNav, manageNav, visibleNav } from './nav';
  import { trashShown } from '$lib/trash-info.svelte';
  import type { Component } from 'svelte';

  type Hit = Schemas['SearchHit'];
  type Item = { id: string; label: string; detail?: string; href: string; icon?: Component; hit?: Hit };

  let el = $state<HTMLDialogElement | null>(null);
  let input = $state<HTMLInputElement | null>(null);
  let q = $state('');
  let hits = $state<Hit[]>([]);
  let searching = $state(false);
  let active = $state(0);
  let seq = 0;

  const pages = $derived(
    visibleNav([...mainNav, ...manageNav, { href: '/settings', label: 'Settings', icon: SettingsIcon }], { admin: !!session.user?.is_admin, trash: trashShown() })
  );
  const items = $derived<Item[]>([
    ...pages
      .filter((p) => p.label.toLowerCase().includes(q.trim().toLowerCase()))
      .map((p) => ({ id: 'page:' + p.href, label: p.label, href: p.href, icon: p.icon })),
    ...hits.map((h) => ({ id: 'file:' + h.path, label: h.name, detail: h.path, href: h.is_dir ? folderHref(h.path) : fileHref(h.path), hit: h }))
  ]);

  $effect(() => {
    const d = el;
    if (!d) return;
    if (palette.open && !d.open) {
      q = '';
      hits = [];
      active = 0;
      d.showModal();
      input?.focus();
    } else if (!palette.open && d.open) d.close();
  });

  let timer: ReturnType<typeof setTimeout>;
  $effect(() => {
    const term = q.trim();
    const my = ++seq;
    clearTimeout(timer);
    active = 0;
    if (term.length < 2) {
      hits = [];
      searching = false;
      return;
    }
    searching = true;
    timer = setTimeout(async () => {
      let r: Hit[] = [];
      try { r = (await api('GET', `/api/search?q=${encodeURIComponent(term)}`)).hits.slice(0, 20); } catch {}
      if (my !== seq) return;
      hits = r;
      searching = false;
    }, 150);
  });

  function choose(it: Item | undefined) {
    if (!it) return;
    palette.open = false;
    goto(it.href);
  }

  function onkeydown(e: KeyboardEvent) {
    // Shortcuts behind the modal (the file list's Delete, Ctrl+A) stay put.
    e.stopPropagation();
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const n = items.length;
      if (n) active = (active + (e.key === 'ArrowDown' ? 1 : n - 1)) % n;
      document.getElementById(`cmd-${active}`)?.scrollIntoView({ block: 'nearest' });
    } else if (e.key === 'Enter') {
      e.preventDefault();
      choose(items[active]);
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      palette.open = false;
    }
  }
</script>

<dialog
  bind:this={el}
  aria-label="Go to"
  class="mx-auto mt-[12vh] w-[calc(100%-2rem)] max-w-lg overflow-hidden rounded-xl border border-border bg-popover p-0 text-popover-foreground shadow-lg backdrop:bg-black/50"
  oncancel={(e) => { e.preventDefault(); palette.open = false; }}
  onclose={() => (palette.open = false)}
  onclick={(e) => { if (e.target === el) palette.open = false; }}
  {onkeydown}
>
  <div class="flex items-center gap-2 border-b border-border px-3">
    <SearchIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
    <input
      bind:this={input}
      bind:value={q}
      type="text"
      role="combobox"
      aria-expanded="true"
      aria-controls="cmd-list"
      aria-activedescendant={items.length ? `cmd-${active}` : undefined}
      aria-label="Go to a page or find a file"
      placeholder="Go to a page or find a file…"
      autocomplete="off"
      spellcheck="false"
      class="h-12 min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
    />
    <kbd class="rounded border border-border px-1.5 text-xs text-muted-foreground max-sm:hidden">Esc</kbd>
  </div>
  <ul id="cmd-list" role="listbox" aria-label="Results" class="max-h-[min(60vh,24rem)] overflow-y-auto p-1.5 empty:hidden">
    {#each items as it, i (it.id)}
      <!-- Keys stay on the input (aria-activedescendant); options are pointer targets. -->
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <li
        id="cmd-{i}"
        role="option"
        aria-selected={i === active}
        class={cn('flex h-11 cursor-pointer items-center gap-3 rounded-md px-2.5 text-sm', i === active && 'bg-accent text-accent-foreground')}
        onpointermove={() => (active = i)}
        onclick={() => choose(it)}
      >
        {#if it.hit}
          <FileIcon entry={it.hit} class="size-5 shrink-0" />
        {:else if it.icon}
          <it.icon class="size-4 shrink-0 text-muted-foreground" />
        {/if}
        <span class="flex min-w-0 flex-1 flex-col">
          <span class="truncate">{it.label}</span>
          {#if it.detail}<small class="truncate text-xs text-muted-foreground">{it.detail}</small>{/if}
        </span>
        {#if it.hit && !it.hit.is_dir}<span class="text-xs text-muted-foreground">{bytes(it.hit.size)}</span>{/if}
      </li>
    {/each}
  </ul>
  <p class={cn('px-4 pb-3 text-xs text-muted-foreground', !items.length && 'py-6 text-center text-sm')} aria-live="polite">
    {searching ? 'Searching files…' : !items.length ? (q.trim() ? 'Nothing found.' : 'Type to search.') : ''}
  </p>
</dialog>
