# Cookbook

## Goal

Use verified Kilovolt configurations and request shapes without relying on
marketing claims.

## Request flow

```text
Client -> authenticated founder backend -> Kilovolt -> AI provider
```

## Prerequisites

Rust stable or Docker, a provider credential for real requests, and `curl`.

## Complete configuration

```bash
export BIND_ADDR=127.0.0.1
export KILOVOLT_PORT=8080
export KILOVOLT_PROJECT_BUDGET=25
export KILOVOLT_DEFAULT_BUDGET=5
export KILOVOLT_DASHBOARD_TOKEN="$(openssl rand -hex 32)"
export KILOVOLT_TELEMETRY_ENABLED=false
```

## Complete runnable code

```bash
cargo build --release
./target/release/kilovolt
```

Then use one of:

- [OpenAI Python](openai-python.md)
- [OpenAI Node.js](openai-node.md)
- [Next.js backend](nextjs-backend.md)
- [$5 per-user budgets](per-user-budgets.md)
- [Project budget](project-budget.md)
- [Pipeline token gates](pipeline-budgets.md)
- [Dashboard security](dashboard-security.md)
- [Docker deployment](docker-deployment.md)
- [Local OpenAI-compatible provider](local-openai-compatible-provider.md)
- [Troubleshooting](troubleshooting.md)

## Verify it works

```bash
curl --fail http://127.0.0.1:8080/health
```

## Expected success behavior

The health route returns `OK`; accepted chat requests reserve then commit prompt
cost and account supported output.

## Expected budget-block behavior

Preflight blocks return structured `429`. A stream already accepted upstream
keeps HTTP `200` headers but stops before the rejected output frame; its local
request record is `429`.

## Security notes

Only the authenticated founder backend may set `X-User-ID`. Keep the gateway
private, protect provider keys, and leave telemetry disabled unless opting in.

## Common failure modes

Missing bearer/content-type headers return `401`/`400`; missing dashboard
configuration returns `503`; unsupported/malformed upstream responses return
`502`; response-header timeout returns `504`.
