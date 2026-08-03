# Docker evaluation and manual deployment

## One-command local evaluation

The release workflow publishes `yodsarun/kilovolt-proxy:latest` for Linux
`amd64` and `arm64`. The image exposes `8080`, listens on `0.0.0.0:8080` when
started without configuration in Docker, and includes a `curl` health check.

```bash
docker run -p 127.0.0.1:8080:8080 yodsarun/kilovolt-proxy:latest
```

Open [http://127.0.0.1:8080](http://127.0.0.1:8080), enter the OpenAI key,
accept or edit the $10 project and $1 default-user calculated-spend limits, and
finish setup. Kilovolt generates the application-facing gateway key. The
provider key is held only in memory, masked in the UI, and substituted upstream
after gateway authentication.

The dashboard's **Send test request** action uses `gpt-4o-mini`, caps output at
16 tokens, and runs through the normal proxy and accounting path. It can incur a
small provider charge. The separate [provider-free demo](../quickstart.md) makes
no paid request.

The host publishing in the command is loopback-only. This evaluation mode has no
separate dashboard login and is not production-secure. Configuration and usage
live only in the running process; restarting, removing, or replacing the
container resets both.

## Evaluation request flow

```text
authenticated application backend
  -> Authorization: Bearer <generated Kilovolt gateway key>
  -> X-User-ID: <identity derived from the authenticated session>
  -> Kilovolt project + user ledger
  -> Authorization: Bearer <temporarily stored OpenAI key>
  -> OpenAI
```

Never accept `X-User-ID` directly from an untrusted browser or mobile client.
The backend must overwrite it with the authenticated user's identity.

## Health and lifecycle

In another terminal:

```bash
curl --fail http://127.0.0.1:8080/health
docker ps
```

The health response is `OK`. Stop the foreground container with Ctrl-C. Starting
a new process displays setup again and begins with an empty ledger.

## Configured manual mode

Existing environment-configured deployments remain supported. When
`KILOVOLT_PROXY_TOKEN` is configured, the trusted backend continues sending:

- the provider credential in `Authorization`;
- the independent proxy credential in `X-Kilovolt-Key`;
- the authenticated accounting identity in `X-User-ID`.

That path forwards the provider authorization instead of storing it through the
browser. Configure `KILOVOLT_DASHBOARD_TOKEN` separately for `/dashboard` and
`/api/stats`. Non-loopback binds also require
`KILOVOLT_ACKNOWLEDGE_PROCESS_LOCAL_LEDGER=true`; that acknowledgement does not
make restarts or replicas safe. See the complete
[configuration reference](../configuration.md) and [security model](../security.md).

For unreleased source changes, build the audited checkout locally:

```bash
docker build -t kilovolt:local .
docker run -p 127.0.0.1:8080:8080 kilovolt:local
```

## Expected budget rejection

Project or user preflight failures return structured `429` responses and appear
as blocked dashboard records. A stream already accepted upstream keeps HTTP
`200` headers but stops before a rejected output frame, with local request status
`429`.

## Common failures

- Port conflicts require choosing a different loopback host port, such as
  `-p 127.0.0.1:18080:8080`, and using that port in the SDK base URL.
- An invalid OpenAI key produces a precise failure from the test request without
  returning the saved key.
- Unknown or stale model pricing fails closed before upstream contact.
- A configured manual container without proxy authentication fails closed on
  its non-loopback internal bind.
