# Troubleshooting

## Goal

Map observable status codes and early stream endings to verified Kilovolt
behavior.

## Request flow

```text
client -> validation/reservation -> upstream acceptance -> bounded body processing
```

## Prerequisites

Access to Kilovolt logs, authenticated `/api/stats`, and a reproducible request
without a real secret in diagnostics.

## Complete configuration

```bash
export RUST_LOG=kilovolt=info
export KILOVOLT_DASHBOARD_TOKEN='diagnostic-secret'
export KILOVOLT_ENABLE_MOCK_UPSTREAM=true
export KILOVOLT_TELEMETRY_ENABLED=false
```

## Complete runnable code

Use the deterministic embedded mock:

```bash
curl --include --no-buffer \
  -H 'Authorization: Bearer mock-key' \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: diagnostic-user' \
  -H 'X-Mock-Upstream: true' \
  -H 'X-Mock-Events: 4' \
  --data '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"test"}],"stream":true}' \
  http://127.0.0.1:8080/v1/chat/completions
```

## Verify it works

Expect HTTP `200`, ordered SSE frames, and `[DONE]`. Then inspect:

```bash
curl --user "kilovolt:${KILOVOLT_DASHBOARD_TOKEN}" \
  http://127.0.0.1:8080/api/stats
```

## Expected success behavior

The request record is `200`, the prompt reservation is finalized, and project
and user committed spend increase together.

## Expected budget-block behavior

- Preflight: HTTP `429`, no upstream contact, no rejected mutation.
- Non-stream preflight: HTTP `429`, combined reservation rejected and no
  upstream contact.
- Non-stream bound violation/unknown post-acceptance usage: HTTP `502`, prompt
  plus full maximum output reservation committed, response withheld.
- Mid-stream: initial HTTP `200`, forwarding stops, local record `429`.

## Security notes

The mock route is for local tests. Do not expose it publicly or paste
authorization values, user IDs, prompts, or logs into public issues.

## Common failure modes

| Symptom | Meaning / check |
|---|---|
| `400` | Invalid JSON/content type, missing non-stream bound, unknown price, or unsupported non-stream Gemini request. |
| `401` | Missing/invalid proxy token, bearer header, or dashboard authentication. |
| `413` | Request exceeded `KILOVOLT_MAX_REQUEST_BODY_BYTES`. |
| `429` | Token gate or calculated project/user budget rejection. |
| `499` in dashboard | Client dropped the response body before normal completion. |
| `502` | Connection/read failure, invalid content type, malformed/oversized upstream body, accounting-bound violation, unknown billable output, or invalid SSE. |
| `503` dashboard | Set `KILOVOLT_DASHBOARD_TOKEN` and restart. |
| `504` | Upstream did not return headers before the configured deadline. |
| Spend reset | The process restarted; persistence is not implemented. |
| Different total than invoice | Pricing/token accounting is an estimate; validate prices and supported fields. |
| Pipeline/day overshoot | Those optional token gates are process-local and not reservation-atomic under concurrency. |
