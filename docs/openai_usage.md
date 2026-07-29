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

`src/budget.rs` contains compiled prefix-based prices and an unknown-model
fallback. Those values were not independently verified in the offline Phase 3
run and may be outdated or wrong for a custom provider. Operators must validate
them before production. Calculated spend is not an exact provider invoice.

## Accounting boundaries

- Prompt input is locally estimated from supported chat messages.
- Non-stream output prefers integer `usage.completion_tokens`, then falls back
  to complete supported message content.
- Streaming output counts supported text per complete SSE event. BPE counts can
  differ from whole-answer tokenization.
- Tool/function-only streaming payloads are not fully covered.
- The financial ledger uses `f64` and is process-local/in-memory.
