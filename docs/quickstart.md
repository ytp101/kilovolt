# 15-minute design-partner evaluation

This path proves Kilovolt's current product boundary on one machine without a
provider account or real API key. It uses the disabled-by-default embedded mock,
fake local credentials, tiny in-memory budgets, and one container.

This page is a provider-free evaluation, not the normal customer installation.
For a real OpenAI-compatible provider, use the one-command browser setup in the
[README quick start](../README.md#quick-start).

## Minute 0–5: start the demo

Prerequisites: Docker with Compose and `curl`.

From a clean checkout:

```bash
docker compose -f docker-compose.demo.yml up -d --build
```

The only published socket is `127.0.0.1:18080`. Compose supplies fake proxy and
dashboard credentials, opts into the process-local ledger acknowledgement needed
inside a container, keeps telemetry off, and enables the local mock explicitly.
No provider request is made.

## Minute 5–10: prove the enforcement path

```bash
./scripts/demo-smoke.sh
```

The script prints a `PASS` line only after asserting each result. It verifies
health, wrong proxy credentials, fail-closed missing pricing, the documented
anonymous identity fallback, a successful bounded request, user and project
preflight `429`s, a stream cut off before `[DONE]`, nonzero spend in authenticated
stats, and dashboard HTML. It also removes its temporary response files on success
or failure.

Try the successful request yourself:

```bash
curl --fail --silent --show-error \
  -H 'Authorization: Bearer fake-provider-key' \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: design-partner-user' \
  -H 'X-Kilovolt-Key: demo-proxy-token' \
  -H 'X-Mock-Upstream: true' \
  --data '{
    "model":"gpt-4o-mini",
    "messages":[{"role":"user","content":"hello"}],
    "stream":false,
    "max_completion_tokens":4
  }' \
  http://127.0.0.1:18080/v1/chat/completions
```

Open [the local dashboard](http://127.0.0.1:18080/dashboard), enter username
`kilovolt` and password `demo-dashboard-token`, or inspect JSON:

```bash
curl --fail --user kilovolt:demo-dashboard-token \
  http://127.0.0.1:18080/api/stats
```

## Minute 10–15: connect a trusted backend

For a real provider, leave `KILOVOLT_ENABLE_MOCK_UPSTREAM=false`, do not send
`X-Mock-Upstream`, replace demo credentials, and pass the provider credential in
`Authorization`. The application change is deliberately small:

- change the SDK base URL from the provider to `http://127.0.0.1:8080/v1`;
- add `X-Kilovolt-Key` when proxy authentication is configured;
- add `X-User-ID` after authenticating the application user;
- bound non-streaming output with `max_completion_tokens` or configure the
  server-side default.

`X-User-ID` is a trusted accounting identity, not authentication. Strip any
client-supplied value and insert the authenticated user ID in the founder's
backend. Kilovolt currently places a missing value in the shared `anonymous`
ledger, so a backend that requires identified users must reject missing identity
before proxying. Never let an untrusted browser or mobile client call Kilovolt
directly with a self-selected ID.

### curl

```bash
export OPENAI_API_KEY='provider-secret'
export KILOVOLT_PROXY_TOKEN='replace-with-server-secret'

curl --fail --show-error \
  -H "Authorization: Bearer ${OPENAI_API_KEY}" \
  -H "X-Kilovolt-Key: ${KILOVOLT_PROXY_TOKEN}" \
  -H 'X-User-ID: authenticated-user-123' \
  -H 'Content-Type: application/json' \
  --data '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Reply with OK"}],"max_completion_tokens":16}' \
  http://127.0.0.1:8080/v1/chat/completions
```

### Python

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

### TypeScript

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

## Supported configuration presets

Kilovolt reads environment variables at startup; these examples use that real
configuration surface rather than introducing a separate demo format:

- [project budget only](../examples/config/project-budget-only.env.example): one
  effective deployment limit;
- [per-user free trial](../examples/config/per-user-free-trial.env.example): one
  project cap plus the same default allowance for each trusted user;
- [streaming enforcement](../examples/config/streaming-enforcement.env.example):
  a small allowance and an early-stream-end client contract.

To try a preset locally, review and replace its placeholder secrets, then load it
before starting Kilovolt:

```bash
set -a
. examples/config/per-user-free-trial.env.example
set +a
cargo run --release
```

Startup is the configuration validation path. Invalid/non-finite/negative budgets,
invalid strict booleans, an invalid pricing file, an invalid output default, an
unsafe public bind, or a malformed/non-HTTP(S) upstream URL stop the process with
a named configuration error. An invalid telemetry URL also stops startup when
telemetry is enabled, but is ignored while telemetry remains off. There is no
separate TOML/YAML config loader or `check --config` command in this phase.

## Stop and roll back

Stop and remove the demo container and network:

```bash
docker compose -f docker-compose.demo.yml down --remove-orphans
```

To roll back an application integration, restore the SDK's original provider base
URL, restore its original provider-key environment setting if that was changed,
and remove the Kilovolt-only `X-Kilovolt-Key` and `X-User-ID` headers. Provider
keys remain provider credentials; Kilovolt never requires moving them into a new
credential store. Run the application's existing provider integration check and
confirm the request reaches the original upstream before removing Kilovolt from
the deployment.

## Current boundary and non-goals

- Cost is a local calculation from supported token fields and configured prices,
  not provider invoice equivalence or a guarantee against every bill.
- Financial ledgers are in memory, reset on restart, and are independent per
  process. Run exactly one Kilovolt process for one logical project budget.
- This slice does not add durable or distributed budgets, hosted control-plane
  behavior, exact billing reconciliation, full provider parity, broad new model
  support, or a production identity system.
- Built-in prices may be stale. Verify them or provide a reviewed pricing file
  before real traffic, and keep provider-side limits and alerts enabled.
- Mid-stream enforcement ends the response before the rejected output frame; HTTP
  headers have already been sent, so the dashboard records `429` while the client
  observes an early end rather than a new HTTP status.
