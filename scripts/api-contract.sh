#!/usr/bin/env bash
# API contract test: Strata (built from this checkout) in Docker with
# Postgres and MinIO, a test user with some files, a mount on S3 and a share,
# then Schemathesis over every operation of the OpenAPI document: no 5xx,
# documented status codes, content types, headers and response schemas, and
# the create -> get -> delete links.
#
#   scripts/api-contract.sh            (needs Docker and uv)
#   EXAMPLES=100 scripts/api-contract.sh
set -euo pipefail
cd "$(dirname "$0")/.."
COMPOSE=(docker compose -f api/contract/compose.yml)
PORT=${STRATA_PORT:-18090}
B="http://127.0.0.1:$PORT"
# Throwaway secrets for this run only.
export STRATA_PORT=$PORT STRATA_SECRET_KEY=$(openssl rand -hex 32) STRATA_ADMIN_PASSWORD=$(openssl rand -hex 16)
export MINIO_USER=contract MINIO_PASSWORD=$(openssl rand -hex 16)
TESTER_PW=$(openssl rand -hex 16)
[ -n "${GITHUB_ACTIONS:-}" ] && echo "::add-mask::$STRATA_ADMIN_PASSWORD" && echo "::add-mask::$MINIO_PASSWORD" && echo "::add-mask::$TESTER_PW"
WORK=$(mktemp -d)
cleanup() {
  status=$?
  [ $status -ne 0 ] && "${COMPOSE[@]}" logs strata | tail -200
  "${COMPOSE[@]}" down -v >/dev/null 2>&1 || true
  rm -rf "$WORK"
  exit $status
}
trap cleanup EXIT

"${COMPOSE[@]}" up -d --build --wait db minio
"${COMPOSE[@]}" up -d --build strata gateway
for _ in $(seq 120); do curl -fsS "$B/healthz" >/dev/null 2>&1 && break; sleep 1; done
curl -fsS "$B/healthz" >/dev/null

json() { python3 -c 'import json,sys; print(json.dumps(dict(zip(sys.argv[1::2], sys.argv[2::2]))))' "$@"; }
api() { # method path [json] -> body; fails on an error status
  curl -fsS -X "$1" -H 'x-strata: 1' -H 'content-type: application/json' ${TOKEN:+-H "Authorization: Bearer $TOKEN"} \
    ${3:+--data-binary "$3"} "$B$2"
}

# The admin makes a test user (not an admin: admin routes answer 403).
TOKEN=$(api POST /api/auth/login "$(json username admin password "$STRATA_ADMIN_PASSWORD")" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')
api POST /api/admin/users "$(json username tester password "$TESTER_PW")" >/dev/null
TOKEN=$(api POST /api/auth/login "$(json username tester password "$TESTER_PW")" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')
[ -n "${GITHUB_ACTIONS:-}" ] && echo "::add-mask::$TOKEN"

# Test data: files at home, a bucket mounted raw, a share.
"${COMPOSE[@]}" exec -T minio sh -c "mc alias set l http://127.0.0.1:9000 $MINIO_USER $MINIO_PASSWORD >/dev/null && mc mb l/contract >/dev/null"
put() { curl -fsS -X PUT -H 'x-strata: 1' -H "Authorization: Bearer $TOKEN" -H 'if-none-match: *' --data-binary "$2" "$B/api/files/content?path=$1" >/dev/null; }
api POST /api/files/mkdir '{"path":"docs"}' >/dev/null
put docs%2Fnotes.txt 'hello'
put readme.md '# readme'
CONN=$(api POST /api/connections "{\"name\":\"bucket\",\"config\":{\"kind\":\"s3\",\"bucket\":\"contract\",\"region\":\"us-east-1\",\"endpoint\":\"http://minio:9000\",\"access_key_id\":\"$MINIO_USER\",\"secret_access_key\":\"$MINIO_PASSWORD\"}}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
api POST /api/mounts "{\"path\":\"bucket\",\"connection_id\":$CONN,\"base_path\":\"\",\"mode\":\"raw\"}" >/dev/null
put bucket%2Fdata.bin 'raw bytes'
api POST /api/shares '{"path":"docs"}' >/dev/null

# Logout would end the session the run uses; /api/ws is a WebSocket.
uvx --from 'schemathesis==4.34.2' schemathesis run "$B/api/v1/openapi.json" --url "$B" \
  -H "Authorization: Bearer $TOKEN" \
  --checks not_a_server_error,status_code_conformance,content_type_conformance,response_schema_conformance,response_headers_conformance \
  --exclude-path /api/auth/logout --exclude-path /api/ws \
  --max-examples "${EXAMPLES:-50}" --no-color
