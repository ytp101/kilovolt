# $5 default per-user budget

## Goal

Give every trusted end-user identity a calculated `$5` process-lifetime limit
inside a larger project limit.

## Request flow

```text
Browser/mobile
    -> authenticated founder backend
    -> trusted X-User-ID
    -> Kilovolt ($100 project, $5 per user)
    -> provider
```

## Prerequisites

An authenticated backend that has a stable internal user ID and a privately
reachable Kilovolt process.

## Complete configuration

```bash
export BIND_ADDR=127.0.0.1
export KILOVOLT_PROJECT_BUDGET=100.00
export KILOVOLT_DEFAULT_BUDGET=5.00
export KILOVOLT_DASHBOARD_TOKEN="$(openssl rand -hex 32)"
export KILOVOLT_TELEMETRY_ENABLED=false
./target/release/kilovolt
```

There is no per-user configuration API in this phase: every distinct trusted
ID receives the same configured default.

## Complete runnable code

```python
import os
import requests

def chat_for_authenticated_user(authenticated_user_id: str, prompt: str):
    response = requests.post(
        "http://127.0.0.1:8080/v1/chat/completions",
        headers={
            "Authorization": f"Bearer {os.environ['OPENAI_API_KEY']}",
            "Content-Type": "application/json",
            "X-User-ID": authenticated_user_id,
        },
        json={
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": prompt}],
            "stream": False,
        },
        timeout=60,
    )
    response.raise_for_status()
    return response.json()

print(chat_for_authenticated_user("database-user-42", "Reply with OK."))
```

## Verify it works

```bash
curl --user "kilovolt:${KILOVOLT_DASHBOARD_TOKEN}" \
  http://127.0.0.1:8080/api/stats
```

Confirm `database-user-42` appears in `current_spend_by_user` and the project
spend includes it.

## Expected success behavior

A request fitting both the remaining `$100` project capacity and `$5` user
capacity is accepted.

## Expected budget-block behavior

The user receives structured `429 User Budget Exceeded` when the next atomic
reservation/charge would pass `$5`, even if project capacity remains. The
rejected increment changes neither ledger.

## Security notes

The browser must not choose `database-user-42`. Authenticate first and insert
the database ID server-side. Otherwise a client can rotate IDs and obtain a new
ledger.

## Common failure modes

Missing IDs share `anonymous`; restarting resets every `$5` ledger; multiple
replicas each grant an independent `$5`; stale prices make the dollar estimate
different from the provider invoice.
