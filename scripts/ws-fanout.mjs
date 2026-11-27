// WebSocket fan-out: N clients of one user on /api/ws, then one change; how
// long until each client hears of it. Prints JSON.
//   node scripts/ws-fanout.mjs <base-url> <token> <clients> <path-to-write>
const [base, token, n = '1000', path = 'fanout.txt'] = process.argv.slice(2);
const N = Number(n);
const url = base.replace(/^http/, 'ws') + '/api/ws';
const headers = { Authorization: `Bearer ${token}` };

const sockets = [];
let t0 = 0;
const heard = [];
let failed = 0;
const open = () =>
    new Promise((resolve) => {
      const ws = new WebSocket(url, { headers });
      sockets.push(ws);
      let first = true;
      ws.onmessage = () => {
        // The first message is the current cursor (connected); the next one
        // is the change.
        if (first) {
          first = false;
          resolve();
        } else if (t0 && !ws.heard) {
          ws.heard = true;
          heard.push(performance.now() - t0);
        }
      };
      ws.onerror = () => {
        failed++;
        resolve();
      };
      setTimeout(resolve, 30_000);
    });
// In waves of 50: a burst of 1,000 connects at once overflows accept queues
// (the server's, or a port proxy's) and measures those instead.
for (let i = 0; i < N; i += 50) await Promise.all(Array.from({ length: Math.min(50, N - i) }, open));
const connected = sockets.filter((s) => s.readyState === WebSocket.OPEN).length;
t0 = performance.now();
const r = await fetch(`${base}/api/files/content?path=${encodeURIComponent(path)}`, {
  method: 'PUT',
  headers: { ...headers, 'x-strata': '1', 'if-none-match': '*' },
  body: String(Date.now())
});
if (!r.ok) throw new Error(`write: ${r.status}`);
const deadline = Date.now() + 15_000;
while (heard.length < connected && Date.now() < deadline) await new Promise((r) => setTimeout(r, 20));
for (const s of sockets) s.close();
heard.sort((a, b) => a - b);
const q = (p) => (heard.length ? Math.round(heard[Math.min(heard.length - 1, Math.floor(p * heard.length))]) : null);
console.log(JSON.stringify({ clients: N, connected, failed, heard: heard.length, p50_ms: q(0.5), p95_ms: q(0.95), p99_ms: q(0.99), max_ms: q(1) }));
process.exit(0);
