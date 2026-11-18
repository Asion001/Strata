<script lang="ts">
  // shadcn-svelte's Input.
  import type { HTMLInputAttributes } from 'svelte/elements';
  import { cn } from '$lib/utils';

  let {
    value = $bindable(),
    ref = $bindable(null),
    class: className,
    ...rest
  }: HTMLInputAttributes & { ref?: HTMLInputElement | null } = $props();
</script>

<input
  bind:this={ref}
  bind:value
  data-slot="input"
  class={cn(
    'flex h-9 w-full min-w-0 rounded-md border border-input bg-transparent px-3 py-1 text-base shadow-xs transition-[color,box-shadow] outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive md:text-sm',
    className
  )}
  {...rest}
/>
