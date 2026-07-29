# Docker deployment

## Goal

Build and run the audited source with a container health check.

## Request flow

```text
private host port -> Kilovolt container -> HTTPS provider
```

## Prerequisites

Docker with BuildKit and enough resources to compile the multi-stage image.

## Complete configuration

```bash
export KILOVOLT_DASHBOARD_TOKEN="$(openssl rand -hex 32)"
export KILOVOLT_PROXY_TOKEN="$(openssl rand -hex 32)"
docker build -t kilovolt:local .
```

## Complete runnable code

```bash
docker run --detach --name kilovolt-test \
  --publish 127.0.0.1:8080:8080 \
  --env BIND_ADDR=0.0.0.0 \
  --env KILOVOLT_ACKNOWLEDGE_PROCESS_LOCAL_LEDGER=true \
  --env KILOVOLT_PROJECT_BUDGET=25 \
  --env KILOVOLT_DEFAULT_BUDGET=5 \
  --env KILOVOLT_PROXY_TOKEN="${KILOVOLT_PROXY_TOKEN}" \
  --env KILOVOLT_DASHBOARD_TOKEN="${KILOVOLT_DASHBOARD_TOKEN}" \
  --env KILOVOLT_TELEMETRY_ENABLED=false \
  kilovolt:local
```

## Verify it works

```bash
curl --fail http://127.0.0.1:8080/health
docker inspect --format '{{.State.Health.Status}}' kilovolt-test
```

## Expected success behavior

The health route returns `OK`; Docker reports `healthy` after the configured
start period.

## Expected budget-block behavior

The same project/user `429` behavior applies inside the container. State is lost
when the process/container restarts.

## Security notes

Publish the host port only on loopback/private interfaces, run exactly one
container for one budget, use a secret manager instead of image layers, and
never bake API keys or tokens into the image. The required acknowledgement does
not make replicas or restarts safe.

## Common failure modes

Cross-architecture dependency installation may require BuildKit/buildx; the
health check needs the included `curl`; published images may not match an
uncommitted source tree, so build the audited revision locally.
