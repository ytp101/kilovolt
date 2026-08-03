#!/usr/bin/env bash
set -euo pipefail

base_url="${KILOVOLT_DEMO_BASE_URL:-http://127.0.0.1:18080}"
base_url="${base_url%/}"
proxy_token="${KILOVOLT_DEMO_PROXY_TOKEN:-demo-proxy-token}"
dashboard_token="${KILOVOLT_DEMO_DASHBOARD_TOKEN:-demo-dashboard-token}"
health_attempts="${KILOVOLT_DEMO_HEALTH_ATTEMPTS:-30}"
health_delay="${KILOVOLT_DEMO_HEALTH_DELAY_SECONDS:-1}"
if [[ -n "${KILOVOLT_DEMO_TMP_PARENT:-}" ]]; then
  [[ -d "${KILOVOLT_DEMO_TMP_PARENT}" ]] || {
    printf '[FAIL] KILOVOLT_DEMO_TMP_PARENT must be an existing directory\n' >&2
    exit 1
  }
  tmp_dir="$(mktemp -d "${KILOVOLT_DEMO_TMP_PARENT%/}/kilovolt-demo.XXXXXX")"
else
  tmp_dir="$(mktemp -d)"
fi

cleanup() {
  rm -rf -- "${tmp_dir}"
}
trap cleanup EXIT

pass() {
  printf '[PASS] %s\n' "$1"
}

fail() {
  printf '[FAIL] %s\n' "$1" >&2
  exit 1
}

require_status() {
  local actual="$1"
  local expected="$2"
  local label="$3"
  local body_file="$4"
  if [[ "${actual}" != "${expected}" ]]; then
    printf 'Response body:\n' >&2
    sed -n '1,80p' "${body_file}" >&2
    fail "${label}: expected HTTP ${expected}, received ${actual}"
  fi
}

command -v curl >/dev/null 2>&1 || fail "curl is required"

healthy=false
for ((attempt = 1; attempt <= health_attempts; attempt += 1)); do
  health_status="$(curl --silent --show-error --output "${tmp_dir}/health" \
    --write-out '%{http_code}' "${base_url}/health" 2>/dev/null || true)"
  if [[ "${health_status}" == "200" ]] && grep -Fxq 'OK' "${tmp_dir}/health"; then
    healthy=true
    break
  fi
  if ((attempt < health_attempts)); then
    sleep "${health_delay}"
  fi
done
[[ "${healthy}" == "true" ]] || fail "Kilovolt is unavailable at ${base_url}"
pass "health endpoint"

common_body='{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hello"}],"stream":false,"max_completion_tokens":4}'

status="$(curl --silent --show-error --output "${tmp_dir}/wrong-token" --write-out '%{http_code}' \
  -H 'Authorization: Bearer fake-provider-key' \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: demo-wrong-token' \
  -H 'X-Kilovolt-Key: wrong-token' \
  -H 'X-Mock-Upstream: true' \
  --data "${common_body}" \
  "${base_url}/v1/chat/completions")"
require_status "${status}" 401 "wrong proxy credential" "${tmp_dir}/wrong-token"
grep -Fq 'kilovolt_proxy_auth_failed' "${tmp_dir}/wrong-token" || fail "wrong proxy credential did not return the expected error"
pass "wrong proxy credential is rejected"

status="$(curl --silent --show-error --output "${tmp_dir}/missing-pricing" --write-out '%{http_code}' \
  -H 'Authorization: Bearer fake-provider-key' \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: demo-missing-pricing' \
  -H "X-Kilovolt-Key: ${proxy_token}" \
  -H 'X-Mock-Upstream: true' \
  --data '{"model":"demo-model-without-pricing","messages":[{"role":"user","content":"hello"}],"stream":false,"max_completion_tokens":4}' \
  "${base_url}/v1/chat/completions")"
require_status "${status}" 400 "missing model pricing" "${tmp_dir}/missing-pricing"
grep -Fq 'model_pricing_not_configured' "${tmp_dir}/missing-pricing" || fail "missing pricing did not fail closed"
pass "missing model pricing fails closed"

status="$(curl --silent --show-error --output "${tmp_dir}/anonymous" --write-out '%{http_code}' \
  -H 'Authorization: Bearer fake-provider-key' \
  -H 'Content-Type: application/json' \
  -H "X-Kilovolt-Key: ${proxy_token}" \
  -H 'X-Mock-Upstream: true' \
  --data "${common_body}" \
  "${base_url}/v1/chat/completions")"
require_status "${status}" 200 "missing trusted user identity fallback" "${tmp_dir}/anonymous"
grep -Fq 'deterministic mock response' "${tmp_dir}/anonymous" || fail "anonymous fallback did not reach the mock"
pass "missing X-User-ID uses the documented anonymous ledger"

status="$(curl --silent --show-error --output "${tmp_dir}/under-budget" --write-out '%{http_code}' \
  -H 'Authorization: Bearer fake-provider-key' \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: demo-under-budget' \
  -H "X-Kilovolt-Key: ${proxy_token}" \
  -H 'X-Mock-Upstream: true' \
  --data "${common_body}" \
  "${base_url}/v1/chat/completions")"
require_status "${status}" 200 "under-budget request" "${tmp_dir}/under-budget"
grep -Fq 'deterministic mock response' "${tmp_dir}/under-budget" || fail "under-budget response was not the deterministic mock"
pass "under-budget request succeeds"

status="$(curl --silent --show-error --output "${tmp_dir}/over-budget" --write-out '%{http_code}' \
  -H 'Authorization: Bearer fake-provider-key' \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: demo-over-budget' \
  -H "X-Kilovolt-Key: ${proxy_token}" \
  -H 'X-Mock-Upstream: true' \
  --data '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hello"}],"stream":false,"max_completion_tokens":16}' \
  "${base_url}/v1/chat/completions")"
require_status "${status}" 429 "over-budget request" "${tmp_dir}/over-budget"
grep -Fq 'User Budget Exceeded' "${tmp_dir}/over-budget" || fail "over-budget response did not identify the user limit"
pass "over-budget request is rejected before upstream contact"

status="$(curl --silent --show-error --output "${tmp_dir}/over-project-budget" --write-out '%{http_code}' \
  -H 'Authorization: Bearer fake-provider-key' \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: demo-over-project-budget' \
  -H "X-Kilovolt-Key: ${proxy_token}" \
  -H 'X-Mock-Upstream: true' \
  --data '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hello"}],"stream":false,"max_completion_tokens":1000}' \
  "${base_url}/v1/chat/completions")"
require_status "${status}" 429 "project over-budget request" "${tmp_dir}/over-project-budget"
grep -Fq 'Project Budget Exceeded' "${tmp_dir}/over-project-budget" || fail "project over-budget response did not identify the project limit"
pass "project-wide over-budget request is rejected before upstream contact"

status="$(curl --silent --show-error --no-buffer --output "${tmp_dir}/stream" --write-out '%{http_code}' \
  -H 'Authorization: Bearer fake-provider-key' \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: demo-stream' \
  -H "X-Kilovolt-Key: ${proxy_token}" \
  -H 'X-Mock-Upstream: true' \
  -H 'X-Mock-Events: 20' \
  --data '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"stream a long answer"}],"stream":true}' \
  "${base_url}/v1/chat/completions")"
require_status "${status}" 200 "streaming request headers" "${tmp_dir}/stream"
grep -Fq 'token-0' "${tmp_dir}/stream" || fail "streaming request returned no output frame"
if grep -Fq '[DONE]' "${tmp_dir}/stream"; then
  fail "stream reached [DONE] instead of being cut off by the budget"
fi
pass "streaming output is cut off at the budget boundary"

status="$(curl --silent --show-error --output "${tmp_dir}/stats" --write-out '%{http_code}' \
  -H "Authorization: Bearer ${dashboard_token}" \
  "${base_url}/api/stats")"
require_status "${status}" 200 "stats endpoint" "${tmp_dir}/stats"
grep -Fq '"multi_instance_safe":false' "${tmp_dir}/stats" || fail "stats did not expose process-local ledger metadata"
grep -Fq '"anonymous":' "${tmp_dir}/stats" || fail "stats did not include the anonymous ledger"
grep -Fq '"demo-under-budget":' "${tmp_dir}/stats" || fail "stats did not include the successful user ledger"
if grep -Eq '"demo-under-budget":0(\.0+)?[,}]' "${tmp_dir}/stats"; then
  fail "stats showed zero spend for the successful request"
fi
grep -Eq '"user_id":"demo-stream".*"status":429' "${tmp_dir}/stats" || fail "stats did not record the streaming cutoff"
pass "authenticated stats expose spend and streaming enforcement"

status="$(curl --silent --show-error --output "${tmp_dir}/dashboard" --write-out '%{http_code}' \
  --user "kilovolt:${dashboard_token}" \
  "${base_url}/dashboard")"
require_status "${status}" 200 "dashboard" "${tmp_dir}/dashboard"
grep -Fq 'Monitor spending' "${tmp_dir}/dashboard" || fail "dashboard HTML was not returned"
pass "authenticated dashboard loads"

printf '\nKilovolt demo smoke passed: health, authentication, pricing, identity, budgets, streaming, stats, and dashboard.\n'
