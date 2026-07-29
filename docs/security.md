# Security

Kilovolt protects a calculated, process-local spending estimate. It is not an
authentication service, secret manager, distributed ledger, or guarantee
against every provider charge.

## Deployment boundary

```text
Untrusted browser/mobile client
        -> authenticated founder backend
        -> trusted X-User-ID + X-Kilovolt-Key + provider credential
        -> private Kilovolt endpoint
        -> AI provider
```

Do not expose `/v1/chat/completions` directly to clients that can choose
`X-User-ID`. The header is retained for per-user accounting, but its value is
trusted rather than authenticated by Kilovolt.

## Threat model

| Threat | Classification | Current control and remaining responsibility |
|---|---|---|
| Forged or rotated `X-User-ID` | Operator responsibility | Only an authenticated backend should insert it. Kilovolt does not verify the identity. |
| Public customer dashboard | Mitigated when configured | `/dashboard` and `/api/stats` require the configured secret through HTTP Basic or bearer auth. Missing configuration disables them. Keep them private and use TLS. |
| Weak dashboard token | Operator responsibility | Generate a high-entropy secret, do not put it in URLs, and rotate it by changing the environment and restarting. |
| Unauthorized proxy use | Mitigated when configured | `KILOVOLT_PROXY_TOKEN` requires a constant-time-checked `X-Kilovolt-Key` before body parsing/reservation. The token is separate from provider and dashboard credentials and is not forwarded to real upstreams. |
| Stolen upstream API key | Partially mitigated | The key is forwarded in memory and is not included in telemetry. Use environment/secret storage, TLS, scoped keys, log controls, and provider rotation. |
| Oversized request body | Mitigated to configured bound | Axum stops reading beyond `KILOVOLT_MAX_REQUEST_BODY_BYTES` and returns structured `413`. |
| Oversized JSON/error response | Mitigated to configured bound | Buffered upstream bodies are limited by `KILOVOLT_MAX_UPSTREAM_BODY_BYTES`. |
| Malformed or unterminated SSE | Mitigated to configured bound | Frames are byte-reconstructed, UTF-8/JSON validated, and limited by `KILOVOLT_MAX_SSE_FRAME_BYTES`; malformed data is not forwarded as valid. |
| Slow upstream before headers | Partially mitigated | A configurable response-header deadline releases the reservation. Streaming after headers has no total-duration deadline. |
| Denial of service | Partially mitigated | Body/frame bounds and optional proxy authentication exist, but there is no request rate or connection limit. Put Kilovolt behind a protected network/reverse proxy. |
| Log leakage | Operator responsibility | Logs include user IDs, model names, request IDs, costs, and errors. Protect log storage and avoid sensitive user IDs. |
| Telemetry privacy | Mitigated by default | Company telemetry is disabled by default and has explicit payload tests. Review fields before opting in. |
| Direct untrusted browser access | Operator responsibility | A browser could supply arbitrary identity and provider credentials. Use a backend-for-frontend. |
| Process restart | Known limitation | All spend, reservations, request history, and token gates reset. |
| Multiple proxy instances | Known limitation | Ledgers are independent; aggregate spend can exceed one configured project limit. |
| Floating-point money | Known limitation | `f64` can reject a decimal edge early or accumulate rounding error. |
| Incorrect/stale model pricing | Operator responsibility | Unknown models fail closed and validated local overrides are supported, but built-ins remain unverified estimates. Validate prices before production. |
| Unknown generated output | Mitigated for inspected OpenAI-compatible choices | Supported text/refusal/function/tool fields are charged before forwarding; unknown non-empty generated fields terminate forwarding. Provider-native schemas outside the documented surface are not claimed. |
| Provider ignores non-stream maximum | Known residual risk | Kilovolt withholds the response and consumes its full internal reservation, but cannot undo provider generation or guarantee the invoice stayed within that reservation. |
| Public mock route | Mitigated by default | The route and `X-Mock-Upstream` are disabled unless explicitly enabled. Configured proxy authentication also applies to the mock route. |
| Company telemetry web-admin authentication | Known limitation | The separate `web/` application has its own deployment and authentication design; operators of that component must set `ADMIN_PASSWORD` and review it independently. |

## Dashboard authentication

Set a random value, for example:

```bash
openssl rand -hex 32
export KILOVOLT_DASHBOARD_TOKEN='generated-value'
```

Browsers receive an HTTP Basic challenge. Use username `kilovolt` and the token
as the password. Automation may use:

```bash
curl --user "kilovolt:${KILOVOLT_DASHBOARD_TOKEN}" \
  http://127.0.0.1:8080/api/stats

curl -H "Authorization: Bearer ${KILOVOLT_DASHBOARD_TOKEN}" \
  http://127.0.0.1:8080/api/stats
```

Basic credentials are only encoding, not encryption. Use HTTPS outside a local
machine and prefer private-network access.

## Proxy authentication

```bash
export KILOVOLT_PROXY_TOKEN="$(openssl rand -hex 32)"
```

The trusted backend sends `X-Kilovolt-Key`; it must still insert its
authenticated `X-User-ID` separately. Proxy authentication proves possession of
one deployment secret, not end-user identity. On a non-loopback bind, Kilovolt
requires this token unless
`KILOVOLT_ALLOW_UNAUTHENTICATED_PUBLIC_PROXY=true` is explicitly set. A reverse
proxy or private network may provide an external control, but Kilovolt cannot
verify it, so the unsafe override remains explicit.

## Secret handling checklist

- Prefer loopback. For any non-loopback bind, acknowledge the process-local
  ledger, configure proxy authentication, and use TLS/network controls.
- Terminate TLS before any provider key or dashboard credential crosses a
  network.
- Remove client-provided `X-User-ID` and insert the authenticated server-side
  identity.
- Do not log request bodies or authorization headers.
- Leave `KILOVOLT_ENABLE_MOCK_UPSTREAM=false` in production.
- Use provider-side spending alerts and limits as a second control.
