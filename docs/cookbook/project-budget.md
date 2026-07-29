# Project-wide budget

## Goal

Enforce one calculated-spend ceiling across all users in the self-hosted
deployment.

## Request flow

```text
many trusted X-User-ID accounts -> one Kilovolt project ledger -> provider
```

## Prerequisites

One Kilovolt process for the project budget domain. Independent replicas are not
safe for a shared aggregate limit.

## Complete configuration

```bash
export KILOVOLT_PROJECT_BUDGET=25.00
export KILOVOLT_DEFAULT_BUDGET=5.00
export KILOVOLT_DASHBOARD_TOKEN="$(openssl rand -hex 32)"
export KILOVOLT_TELEMETRY_ENABLED=false
./target/release/kilovolt
```

## Complete runnable code

```bash
for user in alice bob carol; do
  curl --fail --silent \
    -H "Authorization: Bearer ${OPENAI_API_KEY}" \
    -H 'Content-Type: application/json' \
    -H "X-User-ID: ${user}" \
    --data '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Reply OK"}],"stream":false,"max_completion_tokens":32}' \
    http://127.0.0.1:8080/v1/chat/completions
done
```

## Verify it works

Read authenticated `/api/stats` and compare
`current_project_spend_usd` with the sum of `current_spend_by_user`.

## Expected success behavior

Each request is accepted only when both its user and the shared project retain
capacity.

## Expected budget-block behavior

If project capacity is exhausted first, new users also receive structured
`429 Project Budget Exceeded`; no partial user mutation occurs.

## Security notes

Do not treat this as provider-side billing authority. Retain provider limits and
alerts, and prevent restarts from becoming a way to reset policy.

## Common failure modes

Omitting the project variable makes it fall back to the per-user default;
multiple processes multiply aggregate capacity; floating-point/pricing/token
estimation can diverge from billed spend.
