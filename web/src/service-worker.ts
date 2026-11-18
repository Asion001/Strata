/// <reference types="@sveltejs/kit" />
/// <reference no-default-lib="true"/>
/// <reference lib="esnext" />
/// <reference lib="webworker" />
// Caches the app shell so Strata opens offline / instantly from the home screen.
// API calls and file content always go to the network.
import { build, files, version } from '$service-worker';

const sw = self as unknown as ServiceWorkerGlobalScope;
const CACHE = `strata-shell-${version}`;
// The PDF viewer's large files (worker, stylesheet, CMaps, fonts, WASM) are
// fetched only when a PDF is opened, not precached with the shell.
const lazyPdf = (f: string) => f.startsWith('/pdfjs/') || /\/(pdf\.worker|PdfViewer)[.-]/.test(f);
const ASSETS = [...build, ...files.filter((f) => !f.endsWith('.gz'))].filter((f) => !lazyPdf(f)).concat('/');

sw.addEventListener('install', (event) => {
  event.waitUntil(caches.open(CACHE).then((c) => c.addAll(ASSETS)).then(() => sw.skipWaiting()));
});

sw.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => sw.clients.claim())
  );
});

sw.addEventListener('fetch', (event) => {
  const req = event.request;
  if (req.method !== 'GET') return;
  const url = new URL(req.url);
  if (url.origin !== sw.location.origin || url.pathname.startsWith('/api/')) return;

  // Navigations: network first, fall back to the cached shell.
  if (req.mode === 'navigate') {
    event.respondWith(
      fetch(req).catch(async () => (await caches.match('/')) ?? Response.error())
    );
    return;
  }
  // Static assets: cache first.
  if (ASSETS.includes(url.pathname)) {
    event.respondWith(caches.match(url.pathname).then((r) => r ?? fetch(req)));
  }
});
