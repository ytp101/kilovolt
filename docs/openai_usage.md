# OpenAI route behavior

Non-`gemini-` requests go to `KILOVOLT_OPENAI_UPSTREAM_URL`, which defaults to:

```text
https://api.openai.com/v1/chat/completions
```

The bearer authorization, JSON media type, and original body are forwarded.
The request shape and response accounting limits are documented in
[api_usage.md](api_usage.md).

## Examples

- [Python backend](cookbook/openai-python.md)
- [Node.js backend](cookbook/openai-node.md)
- [Next.js backend-for-frontend](cookbook/nextjs-backend.md)
- [Local OpenAI-compatible endpoint](cookbook/local-openai-compatible-provider.md)

## Pricing warning

Kilovolt has metadata-bearing built-in entries and supports a validated local
operator registry. Unknown models fail closed before upstream; there is no
arbitrary fallback. Built-in values were not independently verified in this
offline work and may be outdated. Operators must validate them before
production. Calculated spend is not an exact provider invoice.

## Accounting boundaries

- Basic prompt input uses the existing message estimator; advanced
  tool/function/structured requests use the maximum of that and a canonical
  billable-request estimate.
- Non-stream output prefers integer `usage.completion_tokens`, then falls back
  to complete supported message content.
- Streaming output counts supported text, refusal, function, tool, and
  structured generated fields per complete SSE event. Unknown non-empty
  generated fields fail closed.
- Non-stream requests reserve a selected maximum output cost before upstream.
- The financial ledger uses `f64` and is process-local/in-memory.
