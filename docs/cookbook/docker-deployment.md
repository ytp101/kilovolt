# Docker deployment

## Goal

Run one normal self-hosted Kilovolt process for a real OpenAI-compatible provider
without designing a Compose file.

## Request flow

```text
authenticated application backend
  -> provider Authorization + Kilovolt proxy secret + trusted user ID
  -> localhost Kilovolt container
  -> HTTPS provider
```

## Prerequisites

Docker with Compose. The default `yodsarun/kilovolt-proxy:v1.3.2` image is pinned
in `.env.example`, published by the repository release workflow for Linux
`amd64` and `arm64`, exposes container port `8080`, and includes its own
`curl`-based health check.

## Complete configuration

```bash
cp .env.example .env
openssl rand -hex 32
openssl rand -hex 32
```

Put the two different generated values in `.env` as `KILOVOLT_PROXY_TOKEN` and
`KILOVOLT_DASHBOARD_TOKEN`. Review both budgets and current model pricing. The
provider API key remains in the trusted application backend; it is sent in the
request's `Authorization` header and is not a Kilovolt environment variable.

To use another reviewed image or tag, change `KILOVOLT_IMAGE` in `.env`. A
published image represents its release tag; build the audited checkout locally
when unreleased source changes are required:

```bash
docker build -t kilovolt:local .
# Then set KILOVOLT_IMAGE=kilovolt:local in .env.
```

The repository `.dockerignore` excludes `.env` and local build state from the
build context.

## Complete runnable code

```bash
docker compose up -d
docker compose ps
curl --fail http://127.0.0.1:8080/health
```

The top-level `docker-compose.yml` publishes only
`127.0.0.1:${KILOVOLT_HOST_PORT:-8080}`, runs one service, inherits the image
health check, and supplies the container-only bind, port, and process-local
ledger acknowledgement. The acknowledgement does not make replicas or restarts
safe.

## Connect the backend

Change the OpenAI SDK base URL to `http://127.0.0.1:8080/v1`. Continue sending
the real provider credential as `Authorization: Bearer ...`, and add:

- `X-Kilovolt-Key`: the proxy secret from `.env`;
- `X-User-ID`: an identity inserted by the authenticated backend.

Proxy and dashboard tokens are separate credentials. Never accept `X-User-ID`
directly from an untrusted browser or mobile client. Complete curl, Python, and
TypeScript examples are in the [README](../../README.md#connect-an-application).

## Verify it works

```bash
curl --fail http://127.0.0.1:8080/health
docker inspect --format '{{.State.Health.Status}}' kilovolt
docker compose logs -f kilovolt
```

The health response is `OK`; Docker reports `healthy` after the configured start
period. Authenticated `/api/stats` requests expose the in-memory spend state.

## Expected budget-block behavior

Project or user preflight failures return structured `429` responses. A stream
already accepted upstream keeps HTTP `200` headers but ends before the rejected
output frame, with local request status `429`.

## Security and state notes

The normal defaults keep the host port on loopback, mock traffic off, telemetry
off, and proxy authentication required by the container's non-loopback bind.
Never enable the unauthenticated public-proxy override for this path. State is in
memory, resets on restart, and is not coordinated across containers; use one
process for one logical project budget. Local calculated costs use `f64`, are not
provider invoice equivalence, and depend on pricing that operators must review.

## Roll back

Restore the application's original provider base URL, remove the Kilovolt-only
headers, retain the original provider-key configuration, and confirm traffic
reaches the provider directly. Then stop Kilovolt:

```bash
docker compose down --remove-orphans
```

## Common failure modes

- Missing or empty `KILOVOLT_PROXY_TOKEN` makes the container fail closed because
  its internal bind is non-loopback.
- A blank dashboard token disables the dashboard and stats routes.
- Invalid strict booleans, pricing files, or deployment exposure fail startup;
  current source also rejects malformed budgets and active URLs.
- Host port conflicts require changing `KILOVOLT_HOST_PORT` in `.env`.
- The provider must support the documented OpenAI-compatible request/response
  shape; a custom URL alone does not guarantee wire compatibility.
