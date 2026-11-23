import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [tailwindcss(), sveltekit()],
  server: {
    proxy: {
      '/api': { target: 'http://localhost:8080', ws: true }
    }
  },
  // Component tests mount the client build of Svelte (in jsdom).
  resolve: process.env.VITEST ? { conditions: ['browser'] } : undefined,
  test: { include: ['src/**/*.test.ts'] }
});
