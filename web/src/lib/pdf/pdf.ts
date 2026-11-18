// pdf.js setup. Imported only by the lazily loaded PDF viewer, so none of
// this is in the startup bundle.
//
// Everything is served from the app's own origin: the worker (a Vite asset)
// and the data files (copied to /pdfjs/<version>/ by scripts/pdfjs-assets.mjs).
// pdf.js 6 never compiles code at runtime (no eval or new Function), so it
// runs under the app's CSP; scripting stays off because no scripting
// manager is ever attached.
import * as pdfjs from 'pdfjs-dist';
import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';
import pkg from 'pdfjs-dist/package.json';

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;
// The viewer components read the library from here.
(globalThis as { pdfjsLib?: typeof pdfjs }).pdfjsLib = pdfjs;

export { pdfjs };

const data = `/pdfjs/${pkg.version}`;

/** Open a PDF from a same-origin URL with range requests: only the parts
 *  needed for the pages on screen are fetched, so a large file shows its
 *  first page long before it would have downloaded. */
export function openPdf(url: string) {
  return pdfjs.getDocument({
    url,
    withCredentials: true,
    // Ranges only, on demand: no full-file stream, no background fetching
    // of the rest of the file.
    disableStream: true,
    disableAutoFetch: true,
    rangeChunkSize: 256 * 1024,
    cMapUrl: `${data}/cmaps/`,
    standardFontDataUrl: `${data}/standard_fonts/`,
    wasmUrl: `${data}/wasm/`,
    iccUrl: `${data}/iccs/`,
    enableXfa: false,
    isOffscreenCanvasSupported: true
  });
}

import { safeExternalUrl } from './links';
export { safeExternalUrl };

/** The pdf.js viewer components, plus a link service that only lets
 *  http(s) links out (new tab, no opener or referrer). Loaded after the
 *  library global above is set, which the components read. */
export async function viewerKit() {
  const v = await import('pdfjs-dist/web/pdf_viewer.mjs');
  class SafeLinkService extends v.PDFLinkService {
    addLinkAttributes(link: HTMLAnchorElement, url: string) {
      const safe = safeExternalUrl(url);
      if (safe) {
        link.href = safe;
        link.title = safe;
        link.target = '_blank';
        link.rel = 'noopener noreferrer';
      } else {
        link.removeAttribute('href');
        link.title = 'Link disabled';
        link.onclick = () => false;
      }
    }
  }
  return { ...v, SafeLinkService };
}
