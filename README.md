# Kilovolt

Kilovolt is a self-hosted Rust gateway placed between an application's trusted
backend and an OpenAI-compatible chat-completions provider. It enforces
process-local calculated-spend limits for the application project and each
trusted `X-User-ID`, including supported streaming output. It complements
provider-side controls; its estimates are not exact provider invoices.

## Install with Docker

Prerequisites: Docker with Compose and `openssl` for generating secrets.

```bash
git clone https://github.com/ytp101/kilovolt.git
cd kilovolt
cp .env.example .env
```

Generate two different secrets and place them in `.env` as
`KILOVOLT_PROXY_TOKEN` and `KILOVOLT_DASHBOARD_TOKEN`:

```bash
openssl rand -hex 32
openssl rand -hex 32
```

Review `KILOVOLT_PROJECT_BUDGET`, `KILOVOLT_DEFAULT_BUDGET`, and the built-in
model prices before sending real traffic. Then start the single Kilovolt service:

```bash
docker compose up -d
docker compose ps
curl --fail http://127.0.0.1:8080/health
```

Expected response:

```text
OK
```

The normal Compose path uses the pinned multi-architecture image configured by
`KILOVOLT_IMAGE`, loads customer settings from `.env`, publishes only to host
loopback, and inherits the image's health check. The container listens on all of
its private interfaces, so Compose supplies the required acknowledgement that
the ledger is process-local; this does not make replicas safe.

Useful lifecycle commands:

```bash
docker compose logs -f kilovolt
docker compose down
```

## Connect an application

Only a trusted, authenticated application backend should call Kilovolt:

```text
Trusted application backend
  -> Authorization: Bearer <provider API key>
  -> X-Kilovolt-Key: <Kilovolt proxy secret>
  -> X-User-ID: <authenticated application user>
  -> Kilovolt
  -> AI provider
```

Change the SDK base URL to `http://127.0.0.1:8080/v1` and add the two Kilovolt
headers. The provider credential remains in the application and continues to be
sent in the request's normal `Authorization` header; it is received and forwarded
by Kilovolt, not copied into Kilovolt's `.env`.

`X-User-ID` is a trusted accounting identity, not authentication. The backend
must strip any client-provided value and insert the ID derived from its
authenticated session. Never let an untrusted browser or mobile client choose
this value or call Kilovolt directly.

### curl

```bash
export OPENAI_API_KEY='provider-secret'
export KILOVOLT_PROXY_TOKEN='same-value-as-KILOVOLT_PROXY_TOKEN-in-.env'

curl --fail --show-error \
  -H "Authorization: Bearer ${OPENAI_API_KEY}" \
  -H "X-Kilovolt-Key: ${KILOVOLT_PROXY_TOKEN}" \
  -H 'X-User-ID: authenticated-user-123' \
  -H 'Content-Type: application/json' \
  --data '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Reply with OK"}],"max_completion_tokens":16}' \
  http://127.0.0.1:8080/v1/chat/completions
```

### Python OpenAI SDK

```python
import os
from openai import OpenAI

client = OpenAI(
    api_key=os.environ["OPENAI_API_KEY"],
    base_url="http://127.0.0.1:8080/v1",
)
response = client.chat.completions.create(
    model="gpt-4o-mini",
    messages=[{"role": "user", "content": "Reply with OK"}],
    max_completion_tokens=16,
    extra_headers={
        "X-Kilovolt-Key": os.environ["KILOVOLT_PROXY_TOKEN"],
        "X-User-ID": "authenticated-user-123",
    },
)
print(response.choices[0].message.content)
```

### TypeScript OpenAI SDK

```typescript
import OpenAI from "openai";

const client = new OpenAI({
  apiKey: process.env.OPENAI_API_KEY,
  baseURL: "http://127.0.0.1:8080/v1",
});
const response = await client.chat.completions.create(
  {
    model: "gpt-4o-mini",
    messages: [{ role: "user", content: "Reply with OK" }],
    max_completion_tokens: 16,
  },
  {
    headers: {
      "X-Kilovolt-Key": process.env.KILOVOLT_PROXY_TOKEN!,
      "X-User-ID": "authenticated-user-123",
    },
  },
);
console.log(response.choices[0].message.content);
```

For non-streaming requests, send a positive `max_completion_tokens` (preferred)
or `max_tokens`, or deliberately size
`KILOVOLT_NON_STREAM_DEFAULT_MAX_OUTPUT_TOKENS`.

## Verify and roll back

Health is public; spend and operational state require the distinct dashboard
credential:

```bash
curl --fail http://127.0.0.1:8080/health
curl --user 'kilovolt:<dashboard-token-from-.env>' \
  http://127.0.0.1:8080/api/stats
```

To remove Kilovolt, first restore the application's original provider base URL
and remove `X-Kilovolt-Key` and `X-User-ID`. Keep using the application's original
provider API-key setting, confirm a request reaches the original upstream, then
stop Kilovolt:

```bash
docker compose down
```

## Provider-free demo

The separate evaluation path uses fake credentials, the explicitly enabled
embedded mock, and tiny budgets. It needs no provider account and makes no paid
provider request:

```bash
docker compose -f docker-compose.demo.yml up -d --build
./scripts/demo-smoke.sh
docker compose -f docker-compose.demo.yml down --remove-orphans
```

See the [15-minute provider-free evaluation](docs/quickstart.md) for the expected
success, rejection, streaming cutoff, and dashboard evidence. This demo is not
the normal real-provider installation above.

## Current limitations

- All accounting is in memory, resets on restart, and is independent per process.
- One logical project budget must use exactly one Kilovolt process; there is no
  multi-replica coordination.
- Money uses `f64`.
- Locally calculated spend is not exact provider invoice equivalence or a
  guarantee against every unexpected bill.
- Built-in prices may be stale and must be reviewed or overridden before real
  traffic.
- Streaming text is accounted per supported SSE event, a bounded conservative
  approximation rather than whole-answer/provider billing equivalence.
- `X-User-ID` is trusted accounting identity, not end-user authentication; a
  missing value uses the shared `anonymous` ledger.
- Kilovolt has no persistence, end-user authentication, rate limiting, or
  distributed consistency.

## Current maturity

Implemented and tested:

- atomic project and per-user prompt reservations and output charges;
- bounded non-streaming prompt-plus-maximum-output reservations with atomic
  settlement or conservative full-reservation finalization;
- bounded OpenAI-compatible SSE reconstruction across arbitrary byte chunks;
- tool/function definitions, calls, refusal, and supported structured-content
  accounting;
- fail-closed, operator-overridable model pricing;
- failure and cancellation-aware prompt lifecycle;
- request, upstream-body, and SSE-frame limits;
- authenticated local customer dashboard;
- company telemetry disabled by default;
- deterministic concurrency, integration, and benchmark harnesses.

Experimental:

- streaming Gemini request/response translation;
- compatibility with a custom OpenAI-like upstream URL, which must be verified
  against the specific provider and version;
- optional pipeline/day token gates, whose check/update lifecycle is weaker
  under concurrency than the financial ledger.

## Supported routes

| Route | Behavior |
|---|---|
| `POST /v1/chat/completions` | OpenAI-shaped streaming and non-streaming requests. Non-Gemini traffic uses the configured OpenAI upstream URL. |
| `GET /health` | Public liveness response: `OK`. |
| `GET /dashboard` | Authenticated local customer dashboard. |
| `GET /api/stats` | Authenticated local JSON statistics. |
| `POST /mock/v1/chat/completions` | Disabled-by-default deterministic local test/benchmark upstream. |

Gemini models are translated only when `stream=true`. Other provider families
and API routes are not claimed.

## Project and per-user budgets

```bash
# One deployment-wide application limit
export KILOVOLT_PROJECT_BUDGET=100.00
# Backward-compatible default applied to each distinct X-User-ID
export KILOVOLT_DEFAULT_BUDGET=5.00
```

Every prompt reservation and output charge checks both levels under one ledger
lock. A failed check leaves both unchanged. If the project variable is omitted,
it falls back to `KILOVOLT_DEFAULT_BUDGET`.

Kilovolt reserves prompt plus maximum output cost for non-streaming requests
before upstream contact, settles exact/provider-reported or supported locally
estimated output afterward, and commits the full output reservation if cost
becomes unknowable after provider acceptance. See
[Safe non-streaming requests](docs/cookbook/safe-non-streaming.md).

## Dashboard

Open `http://127.0.0.1:8080/dashboard`. For the browser's HTTP Basic prompt, use
username `kilovolt` and `KILOVOLT_DASHBOARD_TOKEN` as the password. Without the
token configuration, the dashboard and stats routes return `503`. Use TLS and
private network exposure outside localhost.

## Telemetry

Company telemetry is disabled by default. Explicit opt-in sends the documented
startup, 24-hour process aggregates, and per-recorded-request calculated-cost
payloads to `KILOVOLT_TELEMETRY_URL`. Customer dashboard data remains local and
uses a separate data path. See [telemetry.md](docs/telemetry.md) for exact fields
and privacy implications.

## Verified benchmark snapshot

Three post-fix local runs used an Apple M4, 16 GiB RAM, macOS arm64, release
build, and a loopback deterministic mock. Idle RSS ranged from 10,208–10,256
KiB; tokenizer-active normal workloads reached about 61.5–64.5 MiB; a 512 KiB
prompt peaked at about 93.4–93.6 MiB. Sub-millisecond paired streaming TTFB
differences changed sign across repetitions and are reported as scheduler
noise. A 5,000-event proxied stream added 18.572–18.772 ms p50 end-to-end versus
the direct local mock.

These are one-machine results, not universal guarantees. The raw evidence,
methodology, measurement noise, and a benchmark-discovered tokenizer-cloning
fix are in [benchmarks.md](docs/benchmarks.md). No Go/Python gateway comparison
was performed.

## Documentation

- [Normal Docker deployment](docs/cookbook/docker-deployment.md)
- [15-minute provider-free evaluation](docs/quickstart.md)
- [Architecture and lifecycle](docs/architecture.md)
- [Security and threat model](docs/security.md)
- [Configuration reference](docs/configuration.md)
- [Telemetry fields and controls](docs/telemetry.md)
- [Benchmarks and raw evidence](docs/benchmarks.md)
- [Production checklist](docs/production-checklist.md)
- [Cookbook](docs/cookbook/README.md)
- [API details](docs/api_usage.md)
