# Telemetry and local dashboard data

Kilovolt has two independent data paths.

## Customer-owned operational dashboard

The Rust process keeps local, in-memory state for:

- uptime and calculated average request latency;
- an RSS value (Linux reads `/proc`; non-Linux currently reports a fixed
  placeholder and must not be treated as a measurement);
- total locally counted tokens;
- configured project and default user budgets;
- committed project spend and committed spend by `X-User-ID`;
- the five most recent request records: request ID, time, user ID, model,
  status, duration, tokens, and calculated cost.

This data is served only from `/dashboard` and `/api/stats`. It is not populated
from the Kilovolt company telemetry service. Both routes require
`KILOVOLT_DASHBOARD_TOKEN`, or return `503` when no token is configured.

## Kilovolt company telemetry

Company telemetry is **disabled by default**. With
`KILOVOLT_TELEMETRY_ENABLED=true`, the process sends JSON `POST` requests to
`KILOVOLT_TELEMETRY_URL`, whose default is:

```text
https://kilovolt.vercel.app/v1/update-check
```

The client hash is generated from a random UUID, hashed with SHA-256, and
persisted in `.client_hash` or `/tmp/kilovolt_client_hash` when possible. With
telemetry disabled, startup does not create or load this identifier.

### Startup event

Sent once after process startup:

```json
{
  "type": "startup",
  "client_hash": "persistent anonymous hash",
  "version": "package version",
  "is_docker": true,
  "os": "linux",
  "arch": "amd64"
}
```

### Daily event

The first event is sent after 24 hours, then every 24 hours:

```json
{
  "type": "daily_mapd",
  "client_hash": "persistent anonymous hash",
  "version": "package version",
  "total_requests": 123,
  "total_tokens": 4567,
  "total_users": 12,
  "model_distribution": {
    "gpt-4o-mini": 100
  }
}
```

Counters are process-lifetime values, not a strict previous-24-hour window.

### Per-recorded-request event

Sent asynchronously whenever a local request record is written:

```json
{
  "type": "tsum_update",
  "client_hash": "persistent anonymous hash",
  "cost": 0.0000123
}
```

`cost` is Kilovolt's calculated request estimate. It is not a provider invoice.
The payload does not contain prompts, completions, API keys, request IDs, or
`X-User-ID`. The daily model distribution can reveal model names.

## Enable, redirect, or disable

```bash
# Default and privacy-safe setting
export KILOVOLT_TELEMETRY_ENABLED=false

# Explicit opt-in
export KILOVOLT_TELEMETRY_ENABLED=true

# Test or self-controlled receiver
export KILOVOLT_TELEMETRY_URL=http://127.0.0.1:9000/telemetry
```

All values are read at startup. Changing them requires a restart.

Tests verify that disabled request telemetry makes no receiver request, enabled
request telemetry has exactly the documented fields, and startup/daily payload
builders have the documented shapes.

The repository's `web/` directory is the separate company receiver/dashboard.
It is not needed to run the self-hosted Rust gateway or customer dashboard.
