# Tool and function accounting

## Goal

Use the documented OpenAI-compatible tool/function surface without silently
omitting known billable request or output fields.

## Request flow

```text
tools/messages -> canonical prompt estimate -> provider -> canonical generated fields -> charge -> forward
```

## Prerequisites

A known priced model and upstream behavior matching the documented
chat-completions request/SSE or JSON shapes.

## Complete configuration

```bash
export KILOVOLT_PROJECT_BUDGET=25
export KILOVOLT_DEFAULT_BUDGET=5
```

No switch is required; advanced accounting activates when tool, function,
structured-content, or response-format fields are present.

## Complete runnable code

```bash
curl --no-buffer --fail \
  -H "Authorization: Bearer ${OPENAI_API_KEY}" \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: authenticated-user-123' \
  --data '{
    "model":"gpt-4o-mini",
    "stream":true,
    "messages":[{"role":"user","content":"Weather in Bangkok?"}],
    "tools":[{"type":"function","function":{"name":"forecast","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}}]
  }' \
  http://127.0.0.1:8080/v1/chat/completions
```

## Verify it works

Exercise function names/arguments, tool IDs/types/names/arguments, refusal, and
supported structured content. Confirm spend increases before each corresponding
SSE frame is observable.

## Expected success behavior

Advanced prompt cost is the maximum of the existing message estimate and a
tokenized canonical billable request representation. Supported generated fields
are canonicalized once and charged before forwarding. Non-stream JSON prefers
provider completion usage and otherwise accounts the complete supported message.

## Expected budget-block behavior

The SSE frame that would exceed a project or user limit is neither charged nor
forwarded. Prior prompt/output charges remain committed.

## Security notes

Local tokenization can differ from provider billing. A non-empty generated field
Kilovolt does not classify fails closed and terminates forwarding instead of
silently treating it as free. Known transport metadata such as choice index,
finish reason, and logprobs is passed without being treated as generated text.

## Common failure modes

Unknown generated fields, unsupported structured content types, and malformed
tool/function objects produce an accounting-protocol failure. Provider-native
APIs outside the documented OpenAI-compatible shape are not covered.
