#!/usr/bin/env bash
# Load test: Strata (built from this checkout) in Docker with Postgres, then
# oha against it, and a WebSocket fan-out. Prints a Markdown report (and adds
# it to the GitHub job summary). Reports only: no pass/fail thresholds.
#
#   scripts/load-test.sh              (needs Docker, oha, Node 22, python3)
#   DURATION=10s scripts/load-test.sh
set -euo pipefail
cd "$(dirname "$0")/.."
COMPOSE=(docker compose -f api/load/compose.yml)
PORT=${STRATA_PORT:-18091}
B="http://127.0.0.1:$PORT"
DURATION=${DURATION:-20s}
# Throwaway secrets for this run only.
export STRATA_PORT=$PORT STRATA_SECRET_KEY=$(openssl rand -hex 32) STRATA_ADMIN_PASSWORD=$(openssl rand -hex 16)
[ -n "${GITHUB_ACTIONS:-}" ] && echo "::add-mask::$STRATA_ADMIN_PASSWORD"
WORK=$(mktemp -d)
cleanup() {
  status=$?
  [ $status -ne 0 ] && "${COMPOSE[@]}" logs strata | tail -100
  "${COMPOSE[@]}" down -v >/dev/null 2>&1 || true
  rm -rf "$WORK"
  exit $status
}
trap cleanup EXIT

"${COMPOSE[@]}" up -d --build --wait db
"${COMPOSE[@]}" up -d --build strata
for _ in $(seq 120); do curl -fsS "$B/healthz" >/dev/null 2>&1 && break; sleep 1; done
curl -fsS "$B/healthz" >/dev/null

TOKEN=$(curl -fsS -H 'content-type: application/json' \
  --data-binary "{\"username\":\"admin\",\"password\":\"$STRATA_ADMIN_PASSWORD\"}" "$B/api/auth/login" |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')
[ -n "${GITHUB_ACTIONS:-}" ] && echo "::add-mask::$TOKEN"
AUTH=(-H "Authorization: Bearer $TOKEN")
psql() { "${COMPOSE[@]}" exec -T db psql -qAt -U strata -d strata -c "$1"; }

# Test data: a folder of 100 and one of 10,000 entries (straight into the
# tree, as uploads leave it), a 1 MiB file, and an upload waiting for its
# 1 MiB chunk.
curl -fsS -X POST "${AUTH[@]}" -H 'x-strata: 1' -H 'content-type: application/json' \
  --data-binary '{"path":"warmup"}' "$B/api/files/mkdir" >/dev/null
VOL=$(psql "SELECT volume_id FROM mounts WHERE owner_id = 1 AND path = ''")
psql "INSERT INTO nodes (volume_id, parent, name, is_dir) VALUES ($VOL, '', 'small', TRUE), ($VOL, '', 'big', TRUE);
      INSERT INTO nodes (volume_id, parent, name, is_dir, size, etag)
      SELECT $VOL, 'small', 'f' || i, FALSE, i, md5(i::text) FROM generate_series(1, 100) i;
      INSERT INTO nodes (volume_id, parent, name, is_dir, size, etag)
      SELECT $VOL, 'big', 'f' || i, FALSE, i, md5(i::text) FROM generate_series(1, 10000) i;
      ANALYZE nodes;"
head -c 1048576 /dev/urandom >"$WORK/1MiB"
curl -fsS -X PUT "${AUTH[@]}" -H 'x-strata: 1' -H 'if-none-match: *' --data-binary "@$WORK/1MiB" "$B/api/files/content?path=1MiB.bin" >/dev/null
# The chunk's BLAKE3, with the web app's own implementation.
HASH=$(node --experimental-strip-types --no-warnings web/scripts/blake3-hex.ts "$WORK/1MiB")
UPLOAD=$(curl -fsS -X POST "${AUTH[@]}" -H 'x-strata: 1' -H 'content-type: application/json' \
  --data-binary "{\"path\":\"up.bin\",\"size\":1048576,\"chunks\":[{\"hash\":\"$HASH\",\"size\":1048576}]}" "$B/api/uploads" |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')

run() { # name connections args... -> one table row
  local name=$1 c=$2
  shift 2
  oha --no-tui --output-format json -c "$c" -z "$DURATION" "${AUTH[@]}" -H 'x-strata: 1' "$@" >"$WORK/$name.json"
  python3 - "$name" "$c" "$WORK/$name.json" <<'PY'
import json, sys
name, c, f = sys.argv[1], sys.argv[2], sys.argv[3]
r = json.load(open(f))
s, p = r["summary"], r["latencyPercentiles"]
ok = sum(n for code, n in r.get("statusCodeDistribution", {}).items() if code.startswith("2"))
# (Requests cut off when the run ends are not the server's.)
errors = {k: n for k, n in r.get("errorDistribution", {}).items() if k != "aborted due to deadline"}
total = sum(r.get("statusCodeDistribution", {}).values()) + sum(errors.values())
ms = lambda v: f"{v * 1000:.1f}"
print(f"| {name} | {c} | {s['requestsPerSec']:.0f} | {ms(p['p50'])} | {ms(p['p95'])} | {ms(p['p99'])} | {ok}/{total} |")
PY
}

{
  echo "### Load test ($DURATION per run, this runner)"
  echo
  echo "| Run | Concurrent | Req/s | p50 ms | p95 ms | p99 ms | 2xx/all |"
  echo "|---|---|---|---|---|---|---|"
  run "list 100 entries" 200 "$B/api/files/list?path=small"
  run "list 10,000 entries" 200 "$B/api/files/list?path=big"
  run "download 1 MiB" 50 "$B/api/files/content?path=1MiB.bin"
  run "upload a 1 MiB chunk" 50 -m PUT -D "$WORK/1MiB" "$B/api/uploads/$UPLOAD/chunks/$HASH"
  echo
  FAN=$(node scripts/ws-fanout.mjs "$B" "$TOKEN" 1000 fanout.txt)
  python3 - "$FAN" <<'PY'
import json, sys
r = json.loads(sys.argv[1])
print(f"WebSocket fan-out to {r['clients']} clients of one user, one change: {r['connected']} connected, "
      f"{r['heard']} heard it; p50 {r['p50_ms']} ms, p95 {r['p95_ms']} ms, p99 {r['p99_ms']} ms, max {r['max_ms']} ms")
PY
} | tee -a "${GITHUB_STEP_SUMMARY:-/dev/null}"
