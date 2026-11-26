// Test harness: a files controller with its effects running, on a URL the
// test can change. (Runes only compile in .svelte.ts files.)
import { flushSync } from 'svelte';
import { FilesController } from './files.svelte';

export function harness(start: string) {
  const url = $state({ u: new URL(`http://x${start}`) });
  const c = new FilesController(() => url.u);
  const stop = $effect.root(() => c.start());
  flushSync();
  return {
    c,
    go(to: string) {
      url.u = new URL(`http://x${to}`);
      flushSync();
    },
    stop
  };
}
