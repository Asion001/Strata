<script lang="ts">
  // Native video/audio with remembered position: playback resumes where it
  // stopped last time (per file, in this browser). Only metadata loads up
  // front; the rest streams in byte ranges as it plays or seeks.
  import { onDestroy } from 'svelte';
  import { forget, remember, resumeAt } from './positions';

  let { kind, src, file, name }: { kind: 'video' | 'audio'; src: string; file: string; name: string } = $props();

  let el = $state<HTMLMediaElement>();
  let lastSaved = 0;

  function restore() {
    if (!el) return;
    const t = resumeAt(file, el.duration);
    if (t > 0) el.currentTime = t;
  }
  function save(force = false) {
    if (!el || !Number.isFinite(el.currentTime)) return;
    const now = Date.now();
    if (!force && now - lastSaved < 2000) return;
    lastSaved = now;
    remember(file, el.currentTime);
  }
  onDestroy(() => save(true));
</script>

{#if kind === 'video'}
  <!-- svelte-ignore a11y_media_has_caption -->
  <video bind:this={el} {src} controls playsinline preload="metadata" aria-label={name}
    onloadedmetadata={restore} ontimeupdate={() => save()} onpause={() => save(true)}
    onended={() => forget(file)}></video>
{:else}
  <audio bind:this={el} {src} controls preload="metadata" aria-label={name}
    onloadedmetadata={restore} ontimeupdate={() => save()} onpause={() => save(true)}
    onended={() => forget(file)}></audio>
{/if}

<style>
  video { max-width: 100%; max-height: 100%; object-fit: contain; }
  audio { width: min(640px, 100%); }
</style>
