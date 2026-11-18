<script lang="ts">
  // Shows the toasts from $lib/toast: bottom right on wide screens, above the
  // tab bar on phones. Screen readers hear them through the live regions.
  import { fly } from 'svelte/transition';
  import { toasts, dismissToast } from '$lib/toast.svelte';
  import XIcon from '@lucide/svelte/icons/x';
  import InfoIcon from '@lucide/svelte/icons/info';
  import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
  import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';

  const icons = { info: InfoIcon, success: CircleCheckIcon, error: CircleAlertIcon };
  const tone = { info: 'text-primary', success: 'text-success', error: 'text-destructive' };
</script>

<!-- A polite live region; errors are role="alert", which interrupts. -->
<section
    aria-label="Notifications"
    aria-live="polite"
    class="pointer-events-none fixed inset-x-0 bottom-[calc(72px+env(safe-area-inset-bottom))] z-[70] flex flex-col items-center gap-2 px-4 md:right-[max(1.5rem,env(safe-area-inset-right))] md:bottom-[max(1.5rem,env(safe-area-inset-bottom))] md:left-auto md:items-end md:px-0"
  >
    {#each toasts.list as t (t.id)}
      {@const Icon = icons[t.kind]}
      <div
        transition:fly={{ y: 16, duration: 150 }}
        role={t.kind === 'error' ? 'alert' : 'status'}
        class="pointer-events-auto flex w-full max-w-sm items-start gap-3 rounded-lg border border-border bg-popover p-3 pr-2 text-sm text-popover-foreground shadow-lg"
      >
        <Icon class="mt-0.5 size-4 shrink-0 {tone[t.kind]}" aria-hidden="true" />
        <p class="min-w-0 flex-1 break-words">{t.text}</p>
        <button
          type="button"
          class="-my-1 rounded-md p-1 text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
          aria-label="Dismiss"
          onclick={() => dismissToast(t.id)}
        >
          <XIcon class="size-4" />
        </button>
      </div>
    {/each}
  </section>
