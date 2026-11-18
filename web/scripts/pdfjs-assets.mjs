// Copies pdf.js data files (CMaps, standard fonts, WASM image decoders, ICC
// profiles) into static/, so the PDF viewer loads them from the app's own
// origin. Versioned folder: a pdf.js upgrade never mixes old and new files.
import { cpSync, existsSync, readFileSync, rmSync, writeFileSync } from 'node:fs';

const src = 'node_modules/pdfjs-dist';
const { version } = JSON.parse(readFileSync(`${src}/package.json`, 'utf8'));
const dest = `static/pdfjs/${version}`;
// Written last: a copy that was interrupted is redone.
const done = `${dest}/.complete`;
if (!existsSync(done)) {
  rmSync('static/pdfjs', { recursive: true, force: true });
  for (const d of ['cmaps', 'standard_fonts', 'wasm', 'iccs']) cpSync(`${src}/${d}`, `${dest}/${d}`, { recursive: true });
  writeFileSync(done, '');
}
