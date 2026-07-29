# Local OpenAI-compatible provider

## Goal

Route non-Gemini chat-completions requests to a verified local endpoint instead
of the default OpenAI URL.

## Request flow

```text
trusted backend -> Kilovolt -> local /v1/chat/completions provider
```

## Prerequisites

A provider that actually accepts Kilovolt's forwarded request body and bearer
header and returns either supported SSE or JSON. “OpenAI-compatible” is not one
universal contract; verify your specific server/version.

## Complete configuration

Example for a local server on port 11434:

```bash
export KILOVOLT_OPENAI_UPSTREAM_URL=http://127.0.0.1:11434/v1/chat/completions
export KILOVOLT_PROJECT_BUDGET=25
export KILOVOLT_DEFAULT_BUDGET=5
export KILOVOLT_TELEMETRY_ENABLED=false
./target/release/kilovolt
```

## Complete runnable code

```bash
curl --fail --silent --no-buffer \
  -H 'Authorization: Bearer local-placeholder-if-required' \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: local-user' \
  --data '{
    "model":"provider-specific-model",
    "messages":[{"role":"user","content":"Reply with OK."}],
    "stream":true
  }' \
  http://127.0.0.1:8080/v1/chat/completions
```

## Verify it works

Confirm valid ordered `data:` JSON frames ending in `[DONE]`, then test
`"stream":false` and confirm an intact JSON completion with supported usage or
message content.

## Expected success behavior

Kilovolt uses the configured URL for every non-`gemini-` model and accounts the
supported response shape.

## Expected budget-block behavior

The same atomic project/user limits apply. Unknown model names use Kilovolt's
compiled fallback price, which may be inappropriate for the local provider.

## Security notes

Use TLS when the endpoint is not loopback/private. The URL receives the bearer
credential. Do not assume a local provider's zero-dollar price matches the
fallback.

## Common failure modes

Wrong content type, different SSE/usage schema, tool-call-only output, or a
different endpoint path can produce `502` or inaccurate accounting. Add a
verified model price before financial use.
