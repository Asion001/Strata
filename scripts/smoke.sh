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
# Readable by a containerised STRATA_BIN running as another uid.
chmod 755 "$work"
# The session cookie stays private.
private=$(mktemp -d /tmp/strata-smoke-private.XXXXXX)
dir=""
upload=""
cleanup() {
  # On failure, remove what we created on the server too.
  if [[ -n "$upload" ]]; then c -X DELETE "$BASE/api/uploads/$upload" >/dev/null 2>&1 || true; fi
  if [[ -n "$dir" ]]; then
    c -H 'content-type: application/json' --data "{\"paths\":[\"$dir\"]}" "$BASE/api/files/delete" >/dev/null 2>&1 || true
  fi
  rm -rf "$work" "$private"
}
trap cleanup EXIT
jar="$private/cookies"
c() { curl -sS --fail-with-body ${CURL_EXTRA:-} -b "$jar" -c "$jar" -H 'x-strata: 1' "$@"; }
step() { printf '%-30s ' "$1"; }
ok() { echo "ok${1:+ ($1)}"; }

step "healthz"; c "$BASE/healthz" >/dev/null; ok
json_str() {
  local s=${1//\\/\\\\}
  s=${s//\"/\\\"}
  s=${s//$'\n'/\\n}
  s=${s//$'\r'/\\r}
  s=${s//$'\t'/\\t}
  printf '"%s"' "$s"
}
step "login"
# Body via stdin: the password never appears in the process list.
printf '{"username":%s,"password":%s}' "$(json_str "$STRATA_USER")" "$(json_str "$STRATA_PASSWORD")" |
  c -H 'content-type: application/json' --data @- "$BASE/api/auth/login" >/dev/null
ok

name="smoke-$(date +%s)-$RANDOM"
step "mkdir $name"
c -H 'content-type: application/json' --data "{\"path\":\"$name\"}" "$BASE/api/files/mkdir" >/dev/null
dir="$name"
ok

f="$work/blob.bin"
head -c $((20 * 1024 * 1024)) /dev/urandom > "$f"
chmod 644 "$f"
want=$($BIN hash "$f")
chunks=$($BIN chunk "$f")
spec=$(echo "$chunks" | awk 'BEGIN{printf "["} {printf "%s{\"hash\":\"%s\",\"size\":%s}", (NR>1?",":""), $3, $2} END{printf "]"}')
step "upload 20 MB ($(echo "$chunks" | wc -l | tr -d ' ') chunks)"
t0=$(date +%s)
resp=$(c -H 'content-type: application/json' \
  --data "{\"path\":\"$dir/blob.bin\",\"size\":$(wc -c <"$f" | tr -d ' '),\"chunks\":$spec}" "$BASE/api/uploads")
id=$(echo "$resp" | sed -E 's/.*"id":"([0-9a-f]+)".*/\1/')
upload="$id"
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
upload=""
ok "$(( $(date +%s) - t0 ))s"

step "download + compare"
c "$BASE/api/files/content?path=$dir/blob.bin" -o "$work/back.bin"
chmod 644 "$work/back.bin"
got=$($BIN hash "$work/back.bin")
[[ "$got" == "$want" ]] || { echo "HASH MISMATCH: $got != $want"; exit 1; }
ok "blake3 match"

step "range request"
code=$(curl -sS ${CURL_EXTRA:-} -b "$jar" -o /dev/null -w '%{http_code}' -H 'range: bytes=0-99' "$BASE/api/files/content?path=$dir/blob.bin")
[[ "$code" == 206 ]] || { echo "expected 206, got $code"; exit 1; }
ok

step "delete"
c -H 'content-type: application/json' --data "{\"paths\":[\"$dir\"]}" "$BASE/api/files/delete" >/dev/null
dir=""
ok
echo "smoke test passed"
