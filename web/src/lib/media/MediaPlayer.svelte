<script lang="ts">
  // Native video/audio with remembered position: playback starts (where it
  // stopped last time, per file and user, in this browser) when opened.
  // The rest streams in byte ranges as it plays or seeks.
  import { onDestroy } from 'svelte';
  import { forget, remember, resumeAt } from './positions';

  /** `file`: the key its position is kept under (null: not kept, e.g. on a
   *  public share page). */
  let { kind, src, file, name }: { kind: 'video' | 'audio'; src: string; file: string | null; name: string } = $props();

  let el = $state<HTMLMediaElement>();
  let lastSaved = 0;
  // Nothing is saved before the stored position was applied (closing early
  // would overwrite it with 0), nor after the file played to the end.
  let restored = false;
  let ended = false;

  function restore() {
    if (!el) return;
    if (file) {
      const t = resumeAt(file, el.duration);
      if (t > 0) el.currentTime = t;
    }
    restored = true;
  }
  function save(force = false) {
    if (!file || !el || !restored || ended || !Number.isFinite(el.currentTime)) return;
    const now = Date.now();
    if (!force && now - lastSaved < 2000) return;
    lastSaved = now;
    remember(file, el.currentTime);
  }
  function finished() {
    ended = true;
    if (file) forget(file);
  }
  onDestroy(() => save(true));
</script>

{#if kind === 'video'}
  <!-- svelte-ignore a11y_media_has_caption -->
  <video bind:this={el} {src} controls autoplay playsinline preload="metadata" aria-label={name}
    onloadedmetadata={restore} ontimeupdate={() => save()} onpause={() => save(true)}
    onplay={() => (ended = false)} onended={finished}></video>
{:else}
  <audio bind:this={el} {src} controls autoplay preload="metadata" aria-label={name}
    onloadedmetadata={restore} ontimeupdate={() => save()} onpause={() => save(true)}
    onplay={() => (ended = false)} onended={finished}></audio>
{/if}

<style>
  video { max-width: 100%; max-height: 100%; object-fit: contain; }
  audio { width: min(640px, 100%); }
</style>
