# OpenAI route behavior

All supported requests go to `KILOVOLT_OPENAI_UPSTREAM_URL`, which defaults to:

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
- [Unverified custom upstream compatibility](cookbook/local-openai-compatible-provider.md)

## Pricing warning

Kilovolt supports exact built-in entries for `gpt-4o-mini` and the pinned
`gpt-4o-mini-2024-07-18` snapshot plus a validated local operator registry.
Their $0.15 input / $0.60 output per million text-token prices were verified on
2026-08-29 against the
[official OpenAI model page](https://developers.openai.com/api/docs/models/gpt-4o-mini).
The provider page does not state a separate pricing effective date. Unknown
models fail closed before upstream; there is no arbitrary fallback. Operators
must revalidate pricing before real use. Calculated spend is not an exact
provider invoice.

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
