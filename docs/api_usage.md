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
reconstructs and validates bounded events, charges supported text before
forwarding each event, and recognizes `[DONE]`. If a later charge is rejected,
the already-sent HTTP status stays `200`, forwarding stops, and the local
request record is `429`.

### Non-streaming

```bash
curl \
  -H "Authorization: Bearer ${OPENAI_API_KEY}" \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: user-42' \
  --data '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Hello"}],"stream":false}' \
  http://127.0.0.1:8080/v1/chat/completions
```

A successful upstream must return bounded `application/json`. Kilovolt uses
supported integer `usage.completion_tokens` or tokenizes complete supported
message content as fallback, charges the output, then returns the original JSON
bytes.

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
| `400` | Invalid request headers/JSON or unsupported response mode. |
| `401` | Missing/malformed proxy authorization or dashboard authentication. |
| `413` | Request body exceeded the configured maximum. |
| `429` | Token gate, project budget, or user budget rejected the next operation. |
| `499` | Internal dashboard record for downstream cancellation; not normally an HTTP response. |
| `502` | Upstream connection/body/protocol/content-type failure. |
| `503` | Dashboard token is not configured. |
| `504` | Upstream response headers timed out. |

Non-success upstream statuses and bounded bodies are preserved. They release
the prompt reservation.

## Dashboard routes

`GET /dashboard` and `GET /api/stats` require either browser Basic auth
(`kilovolt` / `KILOVOLT_DASHBOARD_TOKEN`) or:

```bash
curl -H "Authorization: Bearer ${KILOVOLT_DASHBOARD_TOKEN}" \
  http://127.0.0.1:8080/api/stats
```

## Test-only mock

`POST /mock/v1/chat/completions` returns deterministic streaming or JSON output.
`X-Mock-Upstream: true` routes the main proxy to it. `X-Mock-Events` and
`X-Mock-Delay-Ms` control bounded mock behavior. Keep this route private.
