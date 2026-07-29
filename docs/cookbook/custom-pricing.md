# Custom model pricing

## Goal

Configure a verified local price for a model instead of relying on built-ins or
an unsafe fallback.

## Request flow

```text
startup -> validate local registry -> deterministic model match -> budget reservation
```

## Prerequisites

Prices and an effective date independently verified from the operator's provider
contract. Kilovolt does not download pricing.

## Complete configuration

Create `pricing.json`:

```json
{
  "version": 1,
  "entries": [{
    "provider": "openai",
    "model": "custom-local-model",
    "match": "exact",
    "input_usd_per_million_tokens": 0.0,
    "output_usd_per_million_tokens": 0.0,
    "effective_date": "2026-01-01"
  }]
}
```

The zero prices describe a hypothetical free local model; replace all example
metadata with verified values for a billed provider.

## Complete runnable code

```bash
export KILOVOLT_PRICING_FILE="$PWD/pricing.json"
export KILOVOLT_OPENAI_UPSTREAM_URL=http://127.0.0.1:11434/v1/chat/completions
./target/release/kilovolt
```

## Verify it works

Send `custom-local-model` and confirm startup/request logs identify
`operator` pricing. Send a different unknown model and confirm
`model_pricing_not_configured` before the provider sees a request.

## Expected success behavior

Operator exact matches precede explicit prefixes; longest prefix wins; operator
entries override built-ins. Streaming and non-streaming resolve through the same
registry.

## Expected budget-block behavior

Resolved prices feed the same atomic project/user checks. Unknown or ambiguous
pricing is not substituted with another model's price.

## Security notes

Treat the file as financial configuration: review changes, mount it read-only,
and restart deliberately. Kilovolt's estimate is not an exact invoice even when
the per-token price is current.

## Common failure modes

Startup rejects unsupported schema versions, duplicate exact or prefix entries,
ambiguous patterns, empty model names, unknown types, invalid dates, and
negative/non-finite prices.
