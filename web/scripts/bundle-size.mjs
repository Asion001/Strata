// Startup bundle check: the gzipped size of everything the browser loads to
// show the file list (app entry, layout, root page and their static imports).
// Lazy chunks (editor, PDF viewer, ...) don't count.
//
//   node scripts/bundle-size.mjs           compare with bundle-baseline.json
//   node scripts/bundle-size.mjs --update  write the current size as baseline
import { readFileSync, writeFileSync } from 'node:fs';
import { gzipSync } from 'node:zlib';

const out = '.svelte-kit/output/client';
const manifest = JSON.parse(readFileSync(`${out}/.vite/manifest.json`, 'utf8'));
const roots = [
  '.svelte-kit/generated/client-optimized/app.js',
  'node_modules/@sveltejs/kit/src/runtime/client/entry.js',
  '.svelte-kit/generated/client-optimized/nodes/0.js', // root layout
  '.svelte-kit/generated/client-optimized/nodes/2.js' // file list page
];
const seen = new Set();
const files = new Set();
const visit = (key) => {
  if (seen.has(key)) return;
  seen.add(key);
  const m = manifest[key];
  if (!m) throw new Error(`not in the Vite manifest: ${key}`);
  files.add(m.file);
  for (const c of m.css ?? []) files.add(c);
  for (const i of m.imports ?? []) visit(i);
};
roots.forEach(visit);

let gz = 0;
const leaks = [];
for (const f of files) {
  const buf = readFileSync(`${out}/${f}`);
  gz += gzipSync(buf, { level: 9 }).length;
  if (/pdfjs|pdf\.worker|GlobalWorkerOptions/.test(buf.toString('latin1'))) leaks.push(f);
}
const baselineFile = 'bundle-baseline.json';
if (process.argv.includes('--update')) {
  writeFileSync(baselineFile, JSON.stringify({ startupGzipBytes: gz }, null, 2) + '\n');
  console.log(`baseline: ${gz} bytes gzipped (${files.size} files)`);
  process.exit(0);
}
const { startupGzipBytes: base } = JSON.parse(readFileSync(baselineFile, 'utf8'));
// A few hundred bytes of slack: compression shifts a little with any edit.
const slack = 512;
console.log(`startup bundle: ${gz} bytes gzipped (baseline ${base}, ${gz - base >= 0 ? '+' : ''}${gz - base})`);
if (leaks.length) {
  console.error(`pdf.js found in the startup bundle: ${leaks.join(', ')}`);
  process.exit(1);
}
if (gz > base + slack) {
  console.error('the startup bundle grew; move the new code behind a dynamic import (or update the baseline on purpose)');
  process.exit(1);
}
