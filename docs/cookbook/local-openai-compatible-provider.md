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
export KILOVOLT_PRICING_FILE="$PWD/pricing.json"
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

Confirm valid ordered `data:` JSON frames ending in `[DONE]`. For
`"stream":false`, include a positive `max_completion_tokens` and confirm an
intact JSON completion with supported usage or message/function/tool content.

## Expected success behavior

Kilovolt uses the configured URL for every non-`gemini-` model and accounts the
supported response shape.

## Expected budget-block behavior

The same atomic project/user limits apply. Unknown model names fail closed
before upstream; add an exact or explicit prefix entry to the local pricing
registry.

## Security notes

Use TLS when the endpoint is not loopback/private. The URL receives the bearer
credential. Verify any configured zero-dollar local price rather than treating
it as a general fallback.

## Common failure modes

Wrong content type, different SSE/usage schema, unknown generated fields, or a
different endpoint path can produce `502`. Add a verified model price before
financial use.
