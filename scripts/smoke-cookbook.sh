#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="${repo_root}/target/release/kilovolt"
port="${KILOVOLT_SMOKE_PORT:-18080}"
dashboard_token="cookbook-smoke-token"
proxy_token="cookbook-proxy-token"
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

# Non-loopback exposure fails without explicit process-local acknowledgement.
env \
  BIND_ADDR=0.0.0.0 \
  KILOVOLT_PORT="$port" \
  KILOVOLT_TELEMETRY_ENABLED=false \
  "$binary" >"$work_dir/nonloopback.log" 2>&1 &
unsafe_pid=$!
unsafe_exited=false
for _ in {1..100}; do
  if ! kill -0 "$unsafe_pid" 2>/dev/null; then
    unsafe_exited=true
    break
  fi
  sleep 0.01
done
if [[ "$unsafe_exited" != true ]]; then
  kill -TERM "$unsafe_pid" 2>/dev/null || true
  wait "$unsafe_pid" 2>/dev/null || true
  echo "non-loopback startup unexpectedly remained active" >&2
  exit 1
fi
set +e
wait "$unsafe_pid"
unsafe_status=$?
set -e
test "$unsafe_status" -ne 0
grep -q 'Spend resets on restart' "$work_dir/nonloopback.log"

# Mock mode is off by default.
env \
  BIND_ADDR=127.0.0.1 \
  KILOVOLT_PORT="$port" \
  KILOVOLT_PROJECT_BUDGET=10 \
  KILOVOLT_DEFAULT_BUDGET=5 \
  KILOVOLT_TELEMETRY_ENABLED=false \
  RUST_LOG=kilovolt=error \
  "$binary" &
server_pid=$!
for _ in {1..100}; do
  if curl --fail --silent "http://127.0.0.1:${port}/health" >/dev/null; then
    break
  fi
  sleep 0.05
done
mock_disabled_status="$(
  curl --silent --output /dev/null --write-out '%{http_code}' \
    -H 'Content-Type: application/json' --data '{}' \
    "http://127.0.0.1:${port}/mock/v1/chat/completions"
)"
test "$mock_disabled_status" = "404"
kill -TERM "$server_pid"
wait "$server_pid" || true
server_pid=""

env \
  BIND_ADDR=127.0.0.1 \
  KILOVOLT_PORT="$port" \
  KILOVOLT_PROJECT_BUDGET=10 \
  KILOVOLT_DEFAULT_BUDGET=5 \
  KILOVOLT_DASHBOARD_TOKEN="$dashboard_token" \
  KILOVOLT_PROXY_TOKEN="$proxy_token" \
  KILOVOLT_ENABLE_MOCK_UPSTREAM=true \
  KILOVOLT_NON_STREAM_DEFAULT_MAX_OUTPUT_TOKENS=64 \
  KILOVOLT_TELEMETRY_ENABLED=false \
  RUST_LOG=kilovolt=error \
  "$binary" &
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
  "http://127.0.0.1:${port}/api/stats" | grep -q '"multi_instance_safe":false'

stream_payload='{"model":"gpt-4o-mini","messages":[{"role":"user","content":"smoke"}],"stream":true}'
proxy_unauthenticated_status="$(
  curl --silent --output /dev/null --write-out '%{http_code}' \
    -H 'Authorization: Bearer mock-key' \
    -H 'Content-Type: application/json' \
    -H 'X-User-ID: cookbook-user' \
    -H 'X-Mock-Upstream: true' \
    --data "$stream_payload" \
    "http://127.0.0.1:${port}/v1/chat/completions"
)"
test "$proxy_unauthenticated_status" = "401"

curl --fail --silent --no-buffer \
  -H 'Authorization: Bearer mock-key' \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: cookbook-user' \
  -H "X-Kilovolt-Key: ${proxy_token}" \
  -H 'X-Mock-Upstream: true' \
  --data "$stream_payload" \
  "http://127.0.0.1:${port}/v1/chat/completions" | grep -q 'data: \[DONE\]'

json_payload='{"model":"gpt-4o-mini","messages":[{"role":"user","content":"smoke"}],"stream":false}'
curl --fail --silent \
  -H 'Authorization: Bearer mock-key' \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: cookbook-user' \
  -H "X-Kilovolt-Key: ${proxy_token}" \
  -H 'X-Mock-Upstream: true' \
  --data "$json_payload" \
  "http://127.0.0.1:${port}/v1/chat/completions" | grep -q 'deterministic mock response'

unknown_pricing_status="$(
  curl --silent --output /dev/null --write-out '%{http_code}' \
    -H 'Authorization: Bearer mock-key' \
    -H 'Content-Type: application/json' \
    -H 'X-User-ID: cookbook-user' \
    -H "X-Kilovolt-Key: ${proxy_token}" \
    --data '{"model":"unpriced-smoke-model","messages":[{"role":"user","content":"smoke"}],"stream":true}' \
    "http://127.0.0.1:${port}/v1/chat/completions"
)"
test "$unknown_pricing_status" = "400"

echo "cookbook smoke checks passed"
