# Safe non-streaming requests

## Goal

Reserve a bounded worst-case non-streaming output cost before provider
generation begins.

## Request flow

```text
trusted backend -> reserve prompt + maximum output -> provider -> settle actual
```

## Prerequisites

A configured known model price, enough project/user budget for the combined
reservation, and a provider that accepts the selected token-limit field.

## Complete configuration

Prefer an explicit request bound. A deployment fallback is optional:

```bash
export KILOVOLT_NON_STREAM_DEFAULT_MAX_OUTPUT_TOKENS=1000
```

## Complete runnable code

```bash
curl --fail \
  -H "Authorization: Bearer ${OPENAI_API_KEY}" \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: authenticated-user-123' \
  -H "X-Kilovolt-Key: ${KILOVOLT_PROXY_TOKEN}" \
  --data '{
    "model":"gpt-4o-mini",
    "messages":[{"role":"user","content":"Reply briefly."}],
    "stream":false,
    "max_completion_tokens":100
  }' \
  http://127.0.0.1:8080/v1/chat/completions
```

`max_completion_tokens` takes precedence over legacy `max_tokens`, which takes
precedence over the configured default.

## Verify it works

Check authenticated `/api/stats`: after success, reserved spend returns to zero
and committed spend contains prompt plus actual output. Removing all bounds
without a configured default must return `400` with
`non_stream_output_bound_required`.

## Expected success behavior

Kilovolt reserves prompt plus maximum output before upstream, commits prompt at
successful headers, then uses trusted provider usage or a supported local
estimate to commit actual output and release unused capacity.

## Expected budget-block behavior

If the combined reservation does not fit either ledger, Kilovolt returns `429`
without provider contact or ledger mutation. If reported/estimated usage exceeds
the bound, it withholds the response, records `502`, and commits the entire
internal output reservation.

## Security notes

A token maximum can prevent provider generation only when the provider honors
it. A response withheld after generation does not undo provider cost. Malformed,
unsupported, unreadable, or cancelled post-acceptance responses use
conservative full-reservation accounting. None of these modes guarantees exact
invoice equivalence.

## Common failure modes

Zero/non-integer limits and missing bounds return `400`; unknown prices return
`400`; combined budget exhaustion returns `429`; post-acceptance accounting
uncertainty returns `502` and consumes the full output reservation.
