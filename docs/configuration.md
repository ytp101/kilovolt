# Configuration reference

All configuration is read from the environment at startup. Every change in this
table therefore requires a process restart.

| Variable | Type | Default / required | Security and behavior | Example |
|---|---|---|---|---|
| `KILOVOLT_PORT` | `u16` | `8080`, optional | Listening port. Invalid values fall back to the default. | `8080` |
| `BIND_ADDR` | string | `0.0.0.0`, optional | Preferred bind host or full `host:port`. Binding publicly exposes the proxy, health, mock, and authenticated dashboard routes. | `127.0.0.1` |
| `HOST` | string | none, optional legacy fallback | Used only when `BIND_ADDR` is absent. Same exposure implications. | `127.0.0.1` |
| `KILOVOLT_PROJECT_BUDGET` | finite non-negative `f64` USD estimate | Falls back to `KILOVOLT_DEFAULT_BUDGET` | Deployment-wide calculated-spend limit shared by all users. Floating-point limitations apply. | `100.00` |
| `KILOVOLT_DEFAULT_BUDGET` | finite non-negative `f64` USD estimate | `1.00` | Default per-`X-User-ID` calculated-spend limit. No runtime per-user override API exists. | `5.00` |
| `KILOVOLT_OPENAI_UPSTREAM_URL` | absolute HTTP(S) URL | `https://api.openai.com/v1/chat/completions` | Receives provider credentials and request bodies for non-Gemini models. Use TLS outside localhost and verify wire compatibility. | `http://127.0.0.1:11434/v1/chat/completions` |
| `KILOVOLT_UPSTREAM_HEADER_TIMEOUT_SECONDS` | positive integer seconds | `30` | Deadline for receiving upstream response headers. It does not limit a stream after headers. Invalid/zero values use the default. | `15` |
| `KILOVOLT_MAX_REQUEST_BODY_BYTES` | positive integer bytes | `1048576` | Hard bound before JSON allocation; invalid/zero values use the safe default. | `2097152` |
| `KILOVOLT_MAX_UPSTREAM_BODY_BYTES` | positive integer bytes | `4194304` | Bounds non-stream JSON and upstream error bodies. Streaming uses the frame limit instead. | `8388608` |
| `KILOVOLT_MAX_SSE_FRAME_BYTES` | positive integer bytes | `262144` | Maximum buffered single SSE event, including its delimiter. | `131072` |
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
| `Content-Type: application/json` | yes | Required request media type. |
| `X-User-ID` | strongly recommended | Trusted backend identity. Missing values use the shared `anonymous` account. |
| `X-Pipeline-ID` | only for pipeline grouping | Process-local pipeline-run key. |
| `X-Pipeline-Name` | no | Human-readable logging context. |
| `X-Step-Name` | no | Human-readable logging context. |
| `X-Mock-Upstream` | tests only | Routes to the embedded deterministic mock. Do not expose as a production capability. |
| `X-Mock-Events`, `X-Mock-Delay-Ms` | tests only | Control the embedded mock and are capped by the code. |

## Pricing configuration

Pricing is currently compiled into `src/budget.rs`; there is no price
environment variable. The table was not independently verified in this offline
Phase 3 run and can become stale. Unknown models use a fallback. Operators must
validate provider prices before relying on calculated spend. Kilovolt's values
must not be presented as exact invoices.
