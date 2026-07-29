# Pipeline token gates

## Goal

Add process-local prompt, pipeline, and daily token guardrails on top of the
financial ledger.

## Request flow

```text
pipeline run + step headers -> token preflight gates -> financial reservation -> provider
```

## Prerequisites

A backend that generates a stable, unique `X-Pipeline-ID` per pipeline run.

## Complete configuration

```bash
export KILOVOLT_PROJECT_BUDGET=100
export KILOVOLT_DEFAULT_BUDGET=5
export KILOVOLT_PER_STEP_TOKENS=2048
export KILOVOLT_PER_PIPELINE_TOKENS=10000
export KILOVOLT_PER_DAY_TOKENS=100000
./target/release/kilovolt
```

## Complete runnable code

```bash
curl --fail --silent --no-buffer \
  -H "Authorization: Bearer ${OPENAI_API_KEY}" \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: user-42' \
  -H 'X-Pipeline-ID: run-2026-07-29-001' \
  -H 'X-Pipeline-Name: nightly-summary' \
  -H 'X-Step-Name: summarize' \
  --data '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Summarize this."}],"stream":true}' \
  http://127.0.0.1:8080/v1/chat/completions
```

## Verify it works

Repeat requests with the same pipeline ID and inspect `429` bodies/log messages
when configured gates are reached.

## Expected success behavior

Requests below the prompt gate and current process-local estimates proceed to
the atomic project/user financial reservation.

## Expected budget-block behavior

A token gate returns structured `429` with `code=budget_exceeded` before the
financial prompt reservation.

## Security notes

Pipeline headers are trusted context and should be inserted by the backend.
They can influence grouping and logs.

## Common failure modes

The daily/pipeline check and later counter update are not one atomic reservation,
so concurrent requests can race. Missing pipeline ID skips the pipeline check.
Counters reset on restart; daily reset uses server-local midnight.
