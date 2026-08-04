# Kilovolt

Kilovolt is a self-hosted Rust gateway placed between an application's trusted
backend and an OpenAI-compatible chat-completions provider. It enforces
process-local calculated-spend limits for the application project and each
trusted `X-User-ID`, including supported streaming output. It complements
provider-side controls; its estimates are not exact provider invoices.

## Quick start

```bash
docker run -p 127.0.0.1:8080:8080 yodsarun/kilovolt-proxy:latest
```

Kilovolt prints:

```text
╭──────────────────────────────────────────────╮
│  Kilovolt is ready                           │
│                                              │
│  Open the dashboard:                         │
│  http://127.0.0.1:8080                       │
│                                              │
│  Evaluation mode · Data resets on restart    │
╰──────────────────────────────────────────────╯
```

Open [http://127.0.0.1:8080](http://127.0.0.1:8080).

Paste your OpenAI API key, accept or change the spending limits, optionally run
the test request, and open the dashboard's Connect your application panel. No
Git clone, Cargo, Docker Compose, `.env`, or manual secret generation is
required to reach the guided setup.

> **Local evaluation only.** The command publishes the port only on host
> loopback. Evaluation configuration and calculated spend are stored only in
> the running Kilovolt process. Restarting, removing, or replacing the container
> resets setup and usage.

## What happens next

```text
Trusted application backend
  -> Kilovolt gateway key + trusted X-User-ID
  -> Kilovolt checks project and per-user calculated-spend limits
  -> Kilovolt inserts the temporarily stored OpenAI key
  -> OpenAI-compatible provider
```

The setup page pre-fills a $10 project limit and a $1 default per-user limit.
Kilovolt generates a high-entropy gateway key, masks the OpenAI key after setup,
and offers one clearly labeled, very small paid test using `gpt-4o-mini`. A
successful test creates a visible calculated-spend record in the dashboard.

## Connect your application

Copy this `.env` file from the dashboard's expanded Connect your application
panel:

```dotenv
KILOVOLT_API_KEY=kvlt_xxxxxxxxxxxxxxxxx
KILOVOLT_BASE_URL=http://127.0.0.1:8080/v1
```

Do not commit `.env`. Install the Python dependencies:

```bash
pip install openai python-dotenv
```

Then use the environment variables from your trusted backend:

```python
import os

from dotenv import load_dotenv
from openai import OpenAI

load_dotenv()

client = OpenAI(
    api_key=os.environ["KILOVOLT_API_KEY"],
    base_url=os.environ["KILOVOLT_BASE_URL"],
)

response = client.chat.completions.create(
    model="gpt-4o-mini",
    messages=[{"role": "user", "content": "Hello from Kilovolt"}],
    max_completion_tokens=100,
    extra_headers={
        "X-User-ID": "local-test-user",
    },
)

# Optional: confirms the request reached OpenAI through Kilovolt.
print(response.choices[0].message.content)
```

`local-test-user` is for evaluation only. Replace it with the authenticated
user ID from your backend.

`X-User-ID` is a trusted accounting identity, not authentication. A trusted,
authenticated backend must set or overwrite it from the authenticated session.
Browsers and mobile clients must not choose arbitrary user IDs or call this
localhost evaluation gateway directly.

## Current limitations

- Evaluation setup and all calculated spend are temporary in-memory state;
  restarting the process or container resets both.
- One process owns one project budget domain; there is no multi-replica
  coordination.
- Calculated spend is not the exact provider invoice or a guarantee against every
  unexpected bill.
- Built-in pricing can become stale and must be reviewed for real use.
- Monetary calculations currently use floating-point `f64` values.
- Streaming text uses bounded per-event accounting, not exact whole-answer
  provider billing equivalence.
- `X-User-ID` is accounting identity, not end-user authentication; a missing
  value uses the shared `anonymous` ledger.
- The quick start is localhost-only evaluation, not a production-secure
  deployment.

## Provider-free demo

The existing secondary demo uses fake credentials, the explicitly enabled
embedded mock, and tiny budgets. It makes no paid provider request:

```bash
git clone https://github.com/ytp101/kilovolt.git
cd kilovolt
docker compose -f docker-compose.demo.yml up -d --build
./scripts/demo-smoke.sh
docker compose -f docker-compose.demo.yml down --remove-orphans
```

See the [15-minute provider-free evaluation](docs/quickstart.md) for success,
rejection, streaming-cutoff, and dashboard evidence.

## Advanced/manual configuration

The existing configured deployment mode remains available for operators who
set `KILOVOLT_PROXY_TOKEN`: the application sends the provider credential in
`Authorization`, the independent proxy secret in `X-Kilovolt-Key`, and a trusted
`X-User-ID`. Kilovolt forwards the provider credential as before. This legacy
path is distinct from the browser evaluation mode and can use environment
configuration, source builds, custom pricing, and a separately authenticated
dashboard.

See the [configuration reference](docs/configuration.md),
[Docker deployment notes](docs/cookbook/docker-deployment.md), and
[API usage](docs/api_usage.md). For non-streaming requests, provide a positive
`max_completion_tokens` or `max_tokens`, or deliberately configure
`KILOVOLT_NON_STREAM_DEFAULT_MAX_OUTPUT_TOKENS`.

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
- local evaluation dashboard and authenticated configured-mode dashboard;
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
| `GET /` | First-run setup, then the local evaluation dashboard. Configured manual mode redirects to `/dashboard`. |
| `POST /setup` | One-time in-memory evaluation setup; unavailable after completion. |
| `POST /evaluation/test` | Small real-provider test through the normal proxy and accounting path; evaluation mode only. |
| `POST /v1/chat/completions` | OpenAI-shaped streaming and non-streaming requests. Non-Gemini traffic uses the configured OpenAI upstream URL. |
| `GET /health` | Public liveness response: `OK`. |
| `GET /dashboard` | Local evaluation dashboard, or authenticated dashboard in configured manual mode. |
| `GET /api/stats` | Local evaluation statistics, or authenticated statistics in configured manual mode. |
| `POST /mock/v1/chat/completions` | Disabled-by-default deterministic local test/benchmark upstream. |

Configured manual mode translates Gemini models only when `stream=true`.
Browser evaluation mode accepts OpenAI models only so it cannot send the saved
OpenAI key to another provider. Other provider families and API routes are not
claimed.

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

The quick start serves setup and then the dashboard at
`http://127.0.0.1:8080`. Evaluation mode needs no separate admin login because
the documented port is host-loopback only. In configured manual mode, open
`/dashboard` and use username `kilovolt` plus `KILOVOLT_DASHBOARD_TOKEN`; without
that token, dashboard and stats return `503`. Use TLS and private exposure
outside localhost.

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

- [Docker evaluation and manual deployment](docs/cookbook/docker-deployment.md)
- [15-minute provider-free evaluation](docs/quickstart.md)
- [Architecture and lifecycle](docs/architecture.md)
- [Security and threat model](docs/security.md)
- [Configuration reference](docs/configuration.md)
- [Telemetry fields and controls](docs/telemetry.md)
- [Benchmarks and raw evidence](docs/benchmarks.md)
- [Production checklist](docs/production-checklist.md)
- [Cookbook](docs/cookbook/README.md)
- [API details](docs/api_usage.md)
