import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
export default {
  preprocess: vitePreprocess(),
  kit: {
    // Single-page app served by the Rust binary; unknown paths fall back to index.html.
    adapter: adapter({ fallback: 'index.html', precompress: true }),
    serviceWorker: { register: true },
    // Strict CSP for the app itself. The app is a static SPA (no per-request
    // HTML), so inline scripts are allowed by hash instead of nonce; there is
    // no 'unsafe-inline' or 'unsafe-eval' for scripts. 'wasm-unsafe-eval' only
    // permits compiling WebAssembly (BLAKE3 hashing for uploads).
    csp: {
      mode: 'hash',
      directives: {
        'default-src': ['self'],
        'script-src': ['self', 'wasm-unsafe-eval'],
        'style-src': ['self', 'unsafe-inline'],
        'img-src': ['self', 'data:', 'blob:'],
        'media-src': ['self', 'blob:'],
        'connect-src': ['self'],
        'worker-src': ['self'],
        'manifest-src': ['self'],
        'font-src': ['self'],
        'object-src': ['none'],
        'base-uri': ['self'],
        'form-action': ['self']
      }
    }
  }
};
