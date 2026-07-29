#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="${repo_root}/target/release/kilovolt"
port="${KILOVOLT_SMOKE_PORT:-18080}"
dashboard_token="cookbook-smoke-token"
work_dir="$(mktemp -d)"
server_pid=""

cleanup() {
  if [[ -n "$server_pid" ]]; then
    kill -TERM "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -r "$work_dir"
}
trap cleanup EXIT

if [[ ! -x "$binary" ]]; then
  echo "missing release binary; run cargo build --release" >&2
  exit 1
fi

(
  cd "$work_dir"
  BIND_ADDR=127.0.0.1 \
  KILOVOLT_PORT="$port" \
  KILOVOLT_PROJECT_BUDGET=10 \
  KILOVOLT_DEFAULT_BUDGET=5 \
  KILOVOLT_DASHBOARD_TOKEN="$dashboard_token" \
  KILOVOLT_TELEMETRY_ENABLED=false \
  RUST_LOG=kilovolt=error \
  "$binary"
) &
server_pid=$!

for _ in {1..100}; do
  if curl --fail --silent "http://127.0.0.1:${port}/health" >/dev/null; then
    break
  fi
  sleep 0.05
done
curl --fail --silent "http://127.0.0.1:${port}/health" | grep -q '^OK$'

unauthenticated_status="$(
  curl --silent --output /dev/null --write-out '%{http_code}' \
    "http://127.0.0.1:${port}/api/stats"
)"
test "$unauthenticated_status" = "401"
curl --fail --silent --user "kilovolt:${dashboard_token}" \
  "http://127.0.0.1:${port}/api/stats" | grep -q '"project_budget_usd"'

stream_payload='{"model":"gpt-4o-mini","messages":[{"role":"user","content":"smoke"}],"stream":true}'
curl --fail --silent --no-buffer \
  -H 'Authorization: Bearer mock-key' \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: cookbook-user' \
  -H 'X-Mock-Upstream: true' \
  --data "$stream_payload" \
  "http://127.0.0.1:${port}/v1/chat/completions" | grep -q 'data: \[DONE\]'

json_payload='{"model":"gpt-4o-mini","messages":[{"role":"user","content":"smoke"}],"stream":false}'
curl --fail --silent \
  -H 'Authorization: Bearer mock-key' \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: cookbook-user' \
  -H 'X-Mock-Upstream: true' \
  --data "$json_payload" \
  "http://127.0.0.1:${port}/v1/chat/completions" | grep -q 'deterministic mock response'

echo "cookbook smoke checks passed"
