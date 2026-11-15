<script lang="ts">
  import { page } from '$app/state';
  import { api, type Entry } from '$lib/api';
  import { bytes, when, join, icon, isImage } from '$lib/format';
  import Viewer from '$lib/Viewer.svelte';

  const token = $derived(page.params.token);
  let key = $state(sessionStorage.getItem(`share:${page.params.token}`) ?? '');
  let sub = $state('');
  let info = $state<any>(null);
  let error = $state('');
  let password = $state('');
  let viewerIndex = $state(-1);

  const q = (extra = '') => `path=${encodeURIComponent(sub)}${key ? `&k=${key}` : ''}${extra}`;
  const fileUrl = (p: string, dl = false) => `/api/public/${token}/content?path=${encodeURIComponent(p)}${key ? `&k=${key}` : ''}${dl ? '&download=1' : ''}`;

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
  const files = $derived((info?.entries ?? []).filter((e: Entry) => !e.is_dir));
</script>

<div class="page">
  <div class="row brand"><img src="/favicon.png" alt="" width="22" height="22" /><b>Strata</b><span class="muted">shared with you</span></div>
  {#if error}
    <p class="error">{error}</p>
  {:else if info?.needs_password}
    <form class="card stack pw" onsubmit={unlock}>
      <b>🔒 {info.name}</b>
      <label>Password <input type="password" bind:value={password} required /></label>
      <div><button class="primary">Open</button></div>
    </form>
  {:else if info && !info.is_dir}
    <div class="card stack">
      <h1>{info.name}</h1>
      <div class="muted">{bytes(info.size)} · {when(info.mtime)}</div>
      {#if isImage(info.name)}<img class="preview" src={fileUrl('')} alt={info.name} />{/if}
      <div><a href={fileUrl('', true)}><button class="primary">Download</button></a></div>
    </div>
  {:else if info}
    <h1>{info.name}{sub ? ` / ${sub}` : ''}</h1>
    {#if sub}<button onclick={() => (sub = sub.includes('/') ? sub.slice(0, sub.lastIndexOf('/')) : '')}>← Up</button>{/if}
    <table>
      <tbody>
        {#each info.entries as e}
          <tr>
            <td>
              {#if e.is_dir}<a href="#{e.name}" onclick={(ev) => { ev.preventDefault(); sub = join(sub, e.name); }}>{icon(e)} {e.name}</a>
              {:else}<a href="#{e.name}" onclick={(ev) => { ev.preventDefault(); viewerIndex = files.indexOf(e); }}>{icon(e)} {e.name}</a>{/if}
            </td>
            <td class="muted right">{e.is_dir ? '' : bytes(e.size)}</td>
            <td class="right">{#if !e.is_dir}<a href={fileUrl(join(sub, e.name), true)}>Download</a>{/if}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

{#if viewerIndex >= 0}
  <Viewer items={files.map((f: Entry) => ({ name: f.name, size: f.size, src: fileUrl(join(sub, f.name)), download: fileUrl(join(sub, f.name), true) }))}
    bind:index={viewerIndex} onclose={() => (viewerIndex = -1)} />
{/if}

<style>
  .brand { margin-bottom: 16px; } .brand img { border-radius: 5px; }
  .pw { max-width: 360px; }
  .preview { max-width: 100%; max-height: 60vh; border-radius: 8px; object-fit: contain; }
  .right { text-align: right; }
</style>
