# Configuration reference

All configuration is read from the environment at startup. Every change in this
table therefore requires a process restart. Startup fails with a named
configuration error for invalid financial limits, strict security booleans,
pricing files, non-stream output defaults, deployment exposure, or provider URLs.
There is no separate configuration-file schema or validation CLI in this phase.

| Variable | Type | Default / required | Security and behavior | Example |
|---|---|---|---|---|
| `KILOVOLT_PORT` | `u16` | `8080`, optional | Listening port. Invalid values fall back to the default. | `8080` |
| `BIND_ADDR` | string | `127.0.0.1`, optional | Preferred bind host or full `host:port`. Non-loopback startup requires the ledger acknowledgement and proxy authentication or its unsafe override. | `127.0.0.1` |
| `HOST` | string | none, optional legacy fallback | Used only when `BIND_ADDR` is absent. Same exposure implications. | `127.0.0.1` |
| `KILOVOLT_PROJECT_BUDGET` | finite non-negative `f64` USD estimate | Falls back to `KILOVOLT_DEFAULT_BUDGET` | Deployment-wide calculated-spend limit shared by all users. Floating-point limitations apply. | `100.00` |
| `KILOVOLT_DEFAULT_BUDGET` | finite non-negative `f64` USD estimate | `1.00` | Default per-`X-User-ID` calculated-spend limit. No runtime per-user override API exists. | `5.00` |
| `KILOVOLT_OPENAI_UPSTREAM_URL` | absolute HTTP(S) URL | `https://api.openai.com/v1/chat/completions` | Receives provider credentials and request bodies for non-Gemini models. Use TLS outside localhost and verify wire compatibility. | `http://127.0.0.1:11434/v1/chat/completions` |
| `KILOVOLT_UPSTREAM_HEADER_TIMEOUT_SECONDS` | positive integer seconds | `30` | Deadline for receiving upstream response headers. It does not limit a stream after headers. Invalid/zero values use the default. | `15` |
| `KILOVOLT_MAX_REQUEST_BODY_BYTES` | positive integer bytes | `1048576` | Hard bound before JSON allocation; invalid/zero values use the safe default. | `2097152` |
| `KILOVOLT_MAX_UPSTREAM_BODY_BYTES` | positive integer bytes | `4194304` | Bounds non-stream JSON and upstream error bodies. Streaming uses the frame limit instead. | `8388608` |
| `KILOVOLT_MAX_SSE_FRAME_BYTES` | positive integer bytes | `262144` | Maximum buffered single SSE event, including its delimiter. | `131072` |
| `KILOVOLT_NON_STREAM_DEFAULT_MAX_OUTPUT_TOKENS` | positive integer tokens | none | Default reservation bound only when a non-stream request omits both `max_completion_tokens` and `max_tokens`. Without all three, Kilovolt returns `400` before upstream. | `1000` |
| `KILOVOLT_PRICING_FILE` | local JSON file path | none | Validated operator pricing registry. Operator entries override built-ins; startup fails for an invalid file. | `/etc/kilovolt/pricing.json` |
| `KILOVOLT_PROXY_TOKEN` | non-empty secret string | none on loopback | Requires `X-Kilovolt-Key` on proxy and mock POST routes. Required for non-loopback unless the unsafe override is true. Never forwarded to a real provider. | output of `openssl rand -hex 32` |
| `KILOVOLT_ACKNOWLEDGE_PROCESS_LOCAL_LEDGER` | strict boolean | `false` | Required on non-loopback binds. Acknowledges that restart resets spend and replicas have independent budgets; it does not make replicas safe. Invalid values fail startup. | `true` |
| `KILOVOLT_ALLOW_UNAUTHENTICATED_PUBLIC_PROXY` | strict boolean | `false` | Explicit unsafe alternative to `KILOVOLT_PROXY_TOKEN` on non-loopback. It does not supply authentication. Invalid values fail startup. | `false` |
| `KILOVOLT_ENABLE_MOCK_UPSTREAM` | strict boolean | `false` | Enables the embedded test/benchmark route and `X-Mock-Upstream`. Never enable as a production feature. | `true` for local tests only |
| `KILOVOLT_DASHBOARD_TOKEN` | non-empty secret string | none; dashboard disabled | Enables `/dashboard` and `/api/stats`. Use high entropy, TLS, and private exposure. | output of `openssl rand -hex 32` |
| `KILOVOLT_TELEMETRY_ENABLED` | boolean (`true/false`, `1/0`, `yes/no`, `on/off`) | `false` | Explicitly opts into company telemetry. Invalid values use `false`. | `false` |
| `KILOVOLT_TELEMETRY_URL` | absolute HTTP(S) URL | `https://kilovolt.vercel.app/v1/update-check` | Used only when telemetry is enabled. Receives the fields in [telemetry.md](telemetry.md). | `http://127.0.0.1:9000/telemetry` |
| `KILOVOLT_PER_STEP_TOKENS` | positive integer tokens | none | Rejects a prompt whose locally estimated prompt tokens exceed the value. | `2048` |
| `KILOVOLT_PER_PIPELINE_TOKENS` | positive integer tokens | none | Requires `X-Pipeline-ID` to group usage. The check/update is not a financial reservation and is weaker under concurrency. | `10000` |
| `KILOVOLT_PER_DAY_TOKENS` | positive integer tokens | none | Process-local counter reset at server-local midnight. The check/update is not atomic with request execution. | `100000` |
| `RUST_LOG` | tracing filter string | `kilovolt=info,tower_http=debug,axum=info` | Logs can contain user IDs, models, request IDs, costs, and errors. Restrict access. | `kilovolt=info` |

## Request headers

| Header | Required | Meaning |
|---|---|---|
| `Authorization: Bearer …` | yes | Forwarded to OpenAI-compatible upstreams; translated to `x-goog-api-key` for Gemini. |
| `X-Kilovolt-Key` | when `KILOVOLT_PROXY_TOKEN` is configured | Independent gateway credential checked before body parsing or budget reservation. It is not forwarded to real upstreams. |
| `Content-Type: application/json` | yes | Required request media type. |
| `X-User-ID` | strongly recommended | Trusted backend identity. Missing values use the shared `anonymous` account. |
| `X-Pipeline-ID` | only for pipeline grouping | Process-local pipeline-run key. |
| `X-Pipeline-Name` | no | Human-readable logging context. |
| `X-Step-Name` | no | Human-readable logging context. |
| `X-Mock-Upstream` | tests only, mock mode required | Routes to the embedded deterministic mock only when explicitly enabled. |
| `X-Mock-Events`, `X-Mock-Delay-Ms` | tests only | Control the embedded mock and are capped by the code. |

## Pricing configuration

Pricing resolution is fail closed: an unknown model returns
`model_pricing_not_configured` before reservation or upstream contact. Built-in
entries preserve compatibility but were not independently price-verified in
this offline work and have no asserted effective date. Operators must verify
current provider prices.

`KILOVOLT_PRICING_FILE` accepts schema version 1:

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

`provider` is `openai` or `gemini`; `match` is `exact` or `prefix`. Operator
exact matches precede operator prefixes, the longest prefix wins, and operator
entries precede built-ins. Duplicate exact entries, duplicate/ambiguous
prefixes, invalid dates, empty names, unknown types/schema versions, and
negative or non-finite prices fail startup. The example's zero values describe
a hypothetical free local model, not a current provider price.
