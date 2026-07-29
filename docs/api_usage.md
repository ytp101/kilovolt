# API usage

This page is the route-level reference. Start with the
[configuration reference](configuration.md), [security model](security.md), and
[cookbook](cookbook/README.md).

## `GET /health`

Public liveness probe:

```text
HTTP/1.1 200 OK

OK
```

## `POST /v1/chat/completions`

Required headers:

```text
Authorization: Bearer <provider credential>
Content-Type: application/json
```

When configured, `X-Kilovolt-Key` is also required. It is checked before body
parsing and is not forwarded to a real upstream.

Recommended trusted identity header:

```text
X-User-ID: <authenticated backend user ID>
```

`X-User-ID` is not authenticated by Kilovolt. It must be inserted by the
founder's backend after authentication and must not be accepted from an
untrusted browser/mobile client. If missing, all such requests share the
`anonymous` budget.

The request must contain a string `model`, a `messages` array, and optional
boolean `stream` (default `false`). The body is limited by
`KILOVOLT_MAX_REQUEST_BODY_BYTES`.

### Streaming

```bash
curl --no-buffer \
  -H "Authorization: Bearer ${OPENAI_API_KEY}" \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: user-42' \
  --data '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Hello"}],"stream":true}' \
  http://127.0.0.1:8080/v1/chat/completions
```

A successful compatible upstream must return `text/event-stream`. Kilovolt
reconstructs and validates bounded events, charges supported
text/refusal/function/tool fields before forwarding each event, and recognizes
`[DONE]`. Unknown non-empty generated fields terminate forwarding. If a later charge is rejected,
the already-sent HTTP status stays `200`, forwarding stops, and the local
request record is `429`.

### Non-streaming

```bash
curl \
  -H "Authorization: Bearer ${OPENAI_API_KEY}" \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: user-42' \
  --data '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Hello"}],"stream":false,"max_completion_tokens":100}' \
  http://127.0.0.1:8080/v1/chat/completions
```

A positive maximum is selected from `max_completion_tokens`, legacy
`max_tokens`, or the configured default; otherwise the request fails before
upstream. Kilovolt atomically reserves prompt plus maximum output. A successful
upstream must return bounded `application/json`. Trusted integer
`usage.completion_tokens` or a supported complete message estimate settles
actual output and releases the rest. Unknown post-acceptance cost commits the
full output reservation and withholds the response.

Gemini translation requires `stream=true`.

## Errors

Kilovolt-generated errors use:

```json
{
  "error": {
    "message": "User Budget Exceeded",
    "type": "requests",
    "param": null,
    "code": "budget_exceeded"
  }
}
```

| Status | Meaning |
|---:|---|
| `400` | Invalid request, missing output bound, unknown pricing, or unsupported response mode. |
| `401` | Missing/malformed proxy authorization or dashboard authentication. |
| `413` | Request body exceeded the configured maximum. |
| `429` | Token gate, project budget, or user budget rejected the next operation. |
| `499` | Internal dashboard record for downstream cancellation; not normally an HTTP response. |
| `502` | Upstream connection/body/protocol/content-type failure. |
| `503` | Dashboard token is not configured. |
| `504` | Upstream response headers timed out. |

Non-success upstream statuses and bounded bodies are preserved. They release
all pre-acceptance reservations.

## Dashboard routes

`GET /dashboard` and `GET /api/stats` require either browser Basic auth
(`kilovolt` / `KILOVOLT_DASHBOARD_TOKEN`) or:

```bash
curl -H "Authorization: Bearer ${KILOVOLT_DASHBOARD_TOKEN}" \
  http://127.0.0.1:8080/api/stats
```

## Test-only mock

Only `KILOVOLT_ENABLE_MOCK_UPSTREAM=true` enables
`POST /mock/v1/chat/completions` and `X-Mock-Upstream: true`.
`X-Mock-Events` and `X-Mock-Delay-Ms` control bounded mock behavior. Configured
proxy authentication applies. Use this only for local tests and benchmarks.
