# Customer dashboard security

This page documents configured manual mode. The localhost-only browser
evaluation quick start has no separate dashboard login; do not expose that
evaluation port beyond host loopback.

## Goal

Access local operational/budget data without exposing it publicly.

## Request flow

```text
private admin browser/curl -> Basic or bearer token -> Rust /dashboard + /api/stats
```

## Prerequisites

A high-entropy secret and TLS or a local/private-network connection.

## Complete configuration

```bash
export BIND_ADDR=127.0.0.1
export KILOVOLT_DASHBOARD_TOKEN="$(openssl rand -hex 32)"
./target/release/kilovolt
```

Without the token, dashboard routes return `503`, not an open dashboard.

## Complete runnable code

Browser: open `http://127.0.0.1:8080/dashboard`, then enter username
`kilovolt` and the token as the password.

```bash
curl --user "kilovolt:${KILOVOLT_DASHBOARD_TOKEN}" \
  http://127.0.0.1:8080/api/stats

curl -H "Authorization: Bearer ${KILOVOLT_DASHBOARD_TOKEN}" \
  http://127.0.0.1:8080/api/stats
```

## Verify it works

```bash
test "$(curl -s -o /dev/null -w '%{http_code}' \
  http://127.0.0.1:8080/api/stats)" = 401
test "$(curl -s -o /dev/null -w '%{http_code}' \
  --user "kilovolt:${KILOVOLT_DASHBOARD_TOKEN}" \
  http://127.0.0.1:8080/api/stats)" = 200
```

## Expected success behavior

Authenticated access shows uptime, latency, token count, project/user
calculated spend, and five recent records containing request/user/model/status
metadata.

## Expected budget-block behavior

Rejected proxy requests appear in recent records as `429`; the dashboard itself
does not change budgets.

## Security notes

HTTP Basic does not encrypt credentials. Use HTTPS/private access, never put the
token in a query string, and protect the exposed user IDs and usage data.

## Common failure modes

Username must be `kilovolt`; changing the token requires restart; a browser may
cache Basic credentials; reverse proxies must forward `Authorization` and must
not cache `/api/stats`.
