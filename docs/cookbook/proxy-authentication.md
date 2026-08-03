# Proxy authentication

This page documents configured manual mode. The one-command browser evaluation
instead generates a gateway key, accepts it as the SDK bearer key, and substitutes
the temporarily stored OpenAI key upstream.

## Goal

Require a gateway credential before Kilovolt reads a proxy body or reserves
budget.

## Request flow

```text
authenticated client -> founder backend -> X-Kilovolt-Key + trusted X-User-ID -> Kilovolt
```

## Prerequisites

A high-entropy secret delivered to the trusted backend through the deployment's
secret-management mechanism.

## Complete configuration

```bash
export KILOVOLT_PROXY_TOKEN="$(openssl rand -hex 32)"
```

## Complete runnable code

```bash
curl --fail \
  -H "X-Kilovolt-Key: ${KILOVOLT_PROXY_TOKEN}" \
  -H "Authorization: Bearer ${OPENAI_API_KEY}" \
  -H 'Content-Type: application/json' \
  -H 'X-User-ID: authenticated-user-123' \
  --data '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Hi"}],"stream":true}' \
  http://127.0.0.1:8080/v1/chat/completions
```

## Verify it works

The valid request reaches upstream. Missing or wrong `X-Kilovolt-Key` returns
`401`; authenticated stats should show no spend reservation from those failures.

## Expected success behavior

Kilovolt compares the configured secret without data-dependent early exit.
`X-Kilovolt-Key` is not sent to real upstreams. The provider `Authorization`
header remains independent.

## Expected budget-block behavior

Authentication succeeds before normal pricing and budget checks. A later
budget failure remains a structured `429`; authentication failure is `401` and
does not mutate the ledger.

## Security notes

This is deployment-level proxy authentication, not end-user authentication.
The founder backend must authenticate the untrusted client, discard any
client-chosen `X-User-ID`, and insert the trusted identity. Use TLS outside
loopback and keep dashboard/proxy/provider secrets distinct.

## Common failure modes

In configured manual mode, using the proxy secret in `Authorization` sends the
wrong credential upstream.
A reverse proxy may authenticate externally, but non-loopback Kilovolt still
requires a token or the explicit unsafe override because it cannot verify that
external control.
