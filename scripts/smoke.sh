#!/usr/bin/env bash
# Smoke test a running Strata instance end to end:
# healthz, login, create a folder, upload 20 MB with the chunked protocol,
# download it and compare hashes, delete it.
#
#   STRATA_USER=admin STRATA_PASSWORD=... scripts/smoke.sh https://strata.example.com
#
# Optional:
#   CURL_EXTRA="--resolve strata.example.com:443:127.0.0.1"  (test through a local proxy)
#   STRATA_BIN="docker run --rm -v /tmp:/tmp strata:local strata"  (if `strata` is not on PATH;
#                                                                   it provides `chunk` and `hash`)
set -euo pipefail
BASE="${1:?usage: smoke.sh BASE_URL}"
BASE="${BASE%/}"
: "${STRATA_USER:?set STRATA_USER}" "${STRATA_PASSWORD:?set STRATA_PASSWORD}"
BIN="${STRATA_BIN:-strata}"
work=$(mktemp -d /tmp/strata-smoke.XXXXXX)
trap 'rm -rf "$work"' EXIT
jar="$work/cookies"
c() { curl -sS --fail-with-body ${CURL_EXTRA:-} -b "$jar" -c "$jar" -H 'x-strata: 1' "$@"; }
step() { printf '%-30s ' "$1"; }
ok() { echo "ok${1:+ ($1)}"; }

step "healthz"; c "$BASE/healthz" >/dev/null; ok
step "login"
c -H 'content-type: application/json' \
  --data "$(printf '{"username":"%s","password":"%s"}' "$STRATA_USER" "$STRATA_PASSWORD")" \
  "$BASE/api/auth/login" >/dev/null; ok

dir="smoke-$(date +%s)-$RANDOM"
step "mkdir $dir"
c -H 'content-type: application/json' --data "{\"path\":\"$dir\"}" "$BASE/api/files/mkdir" >/dev/null; ok

f="$work/blob.bin"
head -c $((20 * 1024 * 1024)) /dev/urandom > "$f"
want=$($BIN hash "$f")
chunks=$($BIN chunk "$f")
spec=$(echo "$chunks" | awk 'BEGIN{printf "["} {printf "%s{\"hash\":\"%s\",\"size\":%s}", (NR>1?",":""), $3, $2} END{printf "]"}')
step "upload 20 MB ($(echo "$chunks" | wc -l | tr -d ' ') chunks)"
t0=$(date +%s)
resp=$(c -H 'content-type: application/json' \
  --data "{\"path\":\"$dir/blob.bin\",\"size\":$(wc -c <"$f" | tr -d ' '),\"chunks\":$spec}" "$BASE/api/uploads")
id=$(echo "$resp" | sed -E 's/.*"id":"([0-9a-f]+)".*/\1/')
missing=$(echo "$resp" | sed -E 's/.*"missing":\[([0-9,]*)\].*/\1/' | tr ',' ' ')
i=0
while read -r off len hash; do
  if [[ " $missing " == *" $i "* ]]; then
    # (tail gets SIGPIPE once head has enough: not an error)
    (set +o pipefail; tail -c +$((off + 1)) "$f" | head -c "$len" > "$work/part")
    c -X PUT --data-binary @"$work/part" "$BASE/api/uploads/$id/chunks/$hash" >/dev/null
  fi
  i=$((i + 1))
done <<< "$chunks"
c -X POST "$BASE/api/uploads/$id/commit" >/dev/null
ok "$(( $(date +%s) - t0 ))s"

step "download + compare"
c "$BASE/api/files/content?path=$dir/blob.bin" -o "$work/back.bin"
got=$($BIN hash "$work/back.bin")
[[ "$got" == "$want" ]] || { echo "HASH MISMATCH: $got != $want"; exit 1; }
ok "blake3 match"

step "range request"
code=$(curl -sS ${CURL_EXTRA:-} -b "$jar" -o /dev/null -w '%{http_code}' -H 'range: bytes=0-99' "$BASE/api/files/content?path=$dir/blob.bin")
[[ "$code" == 206 ]] || { echo "expected 206, got $code"; exit 1; }
ok

step "delete"
c -H 'content-type: application/json' --data "{\"paths\":[\"$dir\"]}" "$BASE/api/files/delete" >/dev/null; ok
echo "smoke test passed"
