<script lang="ts" module>
  // shadcn-svelte's Button, with the variants as plain class maps (no
  // tailwind-variants dependency, so it stays out of the startup bundle).
  export type ButtonVariant = 'default' | 'destructive' | 'outline' | 'secondary' | 'ghost' | 'link';
  export type ButtonSize = 'default' | 'sm' | 'lg' | 'icon';

  const base =
    "inline-flex shrink-0 cursor-pointer items-center justify-center gap-2 rounded-md text-sm font-medium whitespace-nowrap transition-colors outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4";
  const variants: Record<ButtonVariant, string> = {
    default: 'bg-primary text-primary-foreground shadow-xs hover:bg-primary/90',
    destructive: 'bg-destructive text-destructive-foreground shadow-xs hover:bg-destructive/90 focus-visible:ring-destructive/30',
    outline: 'border border-border bg-background shadow-xs hover:bg-accent hover:text-accent-foreground',
    secondary: 'bg-secondary text-secondary-foreground shadow-xs hover:bg-secondary/80',
    ghost: 'hover:bg-accent hover:text-accent-foreground',
    link: 'text-primary underline-offset-4 hover:underline'
  };
  const sizes: Record<ButtonSize, string> = {
    default: 'h-9 px-4 py-2',
    sm: 'h-8 gap-1.5 px-3',
    lg: 'h-10 px-6',
    icon: 'size-9'
  };

  export function buttonClass(variant: ButtonVariant = 'default', size: ButtonSize = 'default') {
    return `${base} ${variants[variant]} ${sizes[size]}`;
  }
</script>

<script lang="ts">
  import type { HTMLButtonAttributes } from 'svelte/elements';
  import { cn } from '$lib/utils';

  let {
    variant = 'default',
    size = 'default',
    type = 'button',
    class: className,
    ref = $bindable(null),
    children,
    ...rest
  }: HTMLButtonAttributes & { variant?: ButtonVariant; size?: ButtonSize; ref?: HTMLButtonElement | null } = $props();
</script>

<button bind:this={ref} data-slot="button" {type} class={cn(buttonClass(variant, size), className)} {...rest}>
  {@render children?.()}
</button>
