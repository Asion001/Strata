// Byte ranges for files served from the offline cache (video and audio
// seek with ranges, offline too). One range, like the server.

export async function ranged(r: Response, range: string | null): Promise<Response> {
  if (!range) return r;
  const blob = await r.blob();
  const size = blob.size;
  const headers = (extra: Record<string, string>) => {
    const h = new Headers(r.headers);
    for (const [k, v] of Object.entries(extra)) h.set(k, v);
    return h;
  };
  const m = /^bytes=(\d*)-(\d*)$/.exec(range.trim());
  // Not one simple range: the whole file, as the server does.
  if (!m || (m[1] === '' && m[2] === '')) {
    return new Response(blob, { status: 200, headers: headers({ 'content-length': String(size), 'accept-ranges': 'bytes' }) });
  }
  let start: number, end: number; // end exclusive
  if (m[1] === '') {
    const n = Number(m[2]);
    if (n === 0 || size === 0) return unsatisfiable(size, headers);
    start = Math.max(0, size - n);
    end = size;
  } else {
    start = Number(m[1]);
    end = m[2] === '' ? size : Math.min(size, Number(m[2]) + 1);
    if (m[2] !== '' && Number(m[2]) < start) {
      return new Response(blob, { status: 200, headers: headers({ 'content-length': String(size), 'accept-ranges': 'bytes' }) });
    }
    if (start >= size) return unsatisfiable(size, headers);
  }
  return new Response(blob.slice(start, end), {
    status: 206,
    headers: headers({
      'content-range': `bytes ${start}-${end - 1}/${size}`,
      'content-length': String(end - start),
      'accept-ranges': 'bytes'
    })
  });
}

function unsatisfiable(size: number, headers: (e: Record<string, string>) => Headers) {
  return new Response(null, { status: 416, headers: headers({ 'content-range': `bytes */${size}`, 'content-length': '0' }) });
}
