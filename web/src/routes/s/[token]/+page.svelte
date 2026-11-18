<script lang="ts">
  import { page } from '$app/state';
  import { api, pickPreview, type Entry } from '$lib/api';
  import { bytes, when, join, isImage, isSvg } from '$lib/format';
  import { cn } from '$lib/utils';
  import Viewer from '$lib/Viewer.svelte';
  import Button, { buttonClass } from '$lib/components/ui/button/button.svelte';
  import Input from '$lib/components/ui/input/input.svelte';
  import FileIcon from '$lib/components/files/FileIcon.svelte';
  import { cardClass, labelClass, listClass, mutedClass } from '$lib/components/ui/styles';
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import DownloadIcon from '@lucide/svelte/icons/download';
  import LockIcon from '@lucide/svelte/icons/lock';

  const token = $derived(page.params.token);
  let key = $state(sessionStorage.getItem(`share:${page.params.token}`) ?? '');
  let sub = $state('');
  let info = $state<any>(null);
  let error = $state('');
  let password = $state('');
  let viewerIndex = $state(-1);

  const q = (extra = '') => `path=${encodeURIComponent(sub)}${key ? `&k=${key}` : ''}${extra}`;
  const shareUrl = (kind: string, p: string, extra = '') =>
    `/api/public/${token}/${kind}?path=${encodeURIComponent(p)}${key ? `&k=${key}` : ''}${extra}`;
  const imgUrl = (p: string) => shareUrl('image', p);
  const fileUrl = (p: string, dl = false) => shareUrl('content', p, dl ? '&download=1' : '');

  async function load() {
    try { info = await api('GET', `/api/public/${token}?${q()}`); error = ''; }
    catch (e) { error = (e as { status?: number }).status === 404 ? 'This link does not exist or has expired.' : (e as Error).message; }
  }
  $effect(() => { sub; load(); });

  async function unlock(e: Event) {
    e.preventDefault();
    try {
      key = (await api('POST', `/api/public/${token}/unlock`, { password })).key;
      sessionStorage.setItem(`share:${token}`, key);
      await load();
    } catch (err) { error = (err as Error).message; }
  }
  const up = () => (sub = sub.includes('/') ? sub.slice(0, sub.lastIndexOf('/')) : '');
  const files = $derived((info?.entries ?? []).filter((e: Entry) => !e.is_dir));
</script>

<div class="mx-auto min-h-dvh w-full max-w-3xl bg-background px-4 py-6 pb-[calc(1.5rem+env(safe-area-inset-bottom))] text-[15px] text-foreground md:px-8">
  <div class="mb-6 flex items-center gap-2 text-sm">
    <img src="/favicon.png" alt="" width="22" height="22" class="rounded-md" />
    <b class="font-semibold">Strata</b><span class="text-muted-foreground">shared with you</span>
  </div>
  {#if error}
    <p class="text-destructive" role="alert">{error}</p>
  {:else if info?.needs_password}
    <form class={cn(cardClass, 'grid max-w-sm gap-4 p-6')} onsubmit={unlock}>
      <h1 class="flex items-center gap-2 font-semibold"><LockIcon class="size-4" />{info.name}</h1>
      <p class={mutedClass}>This link is protected. Enter its password to open it.</p>
      <label class={labelClass}>Password <Input type="password" bind:value={password} autocomplete="off" required /></label>
      <div><Button type="submit">Open</Button></div>
    </form>
  {:else if info && !info.is_dir}
    <div class={cn(cardClass, 'grid gap-4 p-6')}>
      <div class="flex items-center gap-3">
        <FileIcon entry={{ name: info.name, is_dir: false }} class="size-8 shrink-0" />
        <div class="min-w-0">
          <h1 class="truncate text-lg font-semibold">{info.name}</h1>
          <p class={mutedClass}>{bytes(info.size)} · {when(info.mtime)}</p>
        </div>
      </div>
      {#if isImage(info.name) || isSvg(info.name)}<img class="max-h-[60vh] max-w-full rounded-lg object-contain" src={imgUrl('')} alt={info.name} />{/if}
      <div><a class={buttonClass()} href={fileUrl('', true)}><DownloadIcon />Download</a></div>
    </div>
  {:else if info}
    <div class="mb-4 flex items-center gap-2">
      {#if sub}<Button variant="ghost" size="icon" class="size-8" aria-label="Up one folder" onclick={up}><ArrowLeftIcon /></Button>{/if}
      <h1 class="min-w-0 truncate text-xl font-semibold">{info.name}{sub ? ` / ${sub}` : ''}</h1>
    </div>
    {#if !info.entries.length}
      <p class={mutedClass}>This folder is empty.</p>
    {:else}
      <ul class={listClass}>
        {#each info.entries as e (e.name)}
          <li class="flex items-center gap-3 px-4 py-2.5">
            <FileIcon entry={e} class="size-5 shrink-0" />
            <a href="#{e.name}" class="min-w-0 flex-1 truncate hover:underline"
              onclick={(ev) => { ev.preventDefault(); if (e.is_dir) sub = join(sub, e.name); else viewerIndex = files.indexOf(e); }}>{e.name}</a>
            {#if !e.is_dir}
              <span class="text-sm text-muted-foreground">{bytes(e.size)}</span>
              <a href={fileUrl(join(sub, e.name), true)} class={cn(buttonClass('ghost', 'icon'), 'size-8')} aria-label="Download {e.name}"><DownloadIcon /></a>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</div>

<!-- The viewer isn't rebuilt yet: it keeps the old styles. -->
{#if viewerIndex >= 0}
  <div class="legacy contents">
  <Viewer items={files.map((f: Entry) => ({ name: f.name, size: f.size, src: pickPreview(f.name, fileUrl(join(sub, f.name)), imgUrl(join(sub, f.name))), download: fileUrl(join(sub, f.name), true) }))}
    bind:index={viewerIndex} onclose={() => (viewerIndex = -1)} />
  </div>
{/if}
