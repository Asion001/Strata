import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
export default {
  preprocess: vitePreprocess(),
  kit: {
    // Single-page app served by the Rust binary; unknown paths fall back to index.html.
    adapter: adapter({ fallback: 'index.html', precompress: true }),
    serviceWorker: { register: true }
  }
};
