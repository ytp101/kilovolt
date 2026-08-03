# Production checklist

## Before deployment

- [ ] Read [security.md](security.md) and [architecture.md](architecture.md).
- [ ] Put Kilovolt behind the authenticated founder backend.
- [ ] Strip any client-supplied `X-User-ID`, then insert the authenticated user
      identifier server-side.
- [ ] Bind privately or place a TLS reverse proxy/firewall in front.
- [ ] Configure a high-entropy `KILOVOLT_PROXY_TOKEN` and send it only from the
      trusted backend. Keep it distinct from provider/dashboard credentials.
- [ ] If binding beyond loopback, set
      `KILOVOLT_ACKNOWLEDGE_PROCESS_LOCAL_LEDGER=true` only after accepting
      restart reset and single-process constraints. Do not use the unsafe
      unauthenticated override unless another reviewed control requires it.
- [ ] Generate `KILOVOLT_DASHBOARD_TOKEN` with at least 32 random bytes.
- [ ] Keep `.env` uncommitted and out of the Docker build context; start from the
      reviewed `.env.example` and keep proxy/dashboard values distinct.
- [ ] Set a project budget and a per-user default budget.
- [ ] Validate built-in prices or supply a validated `KILOVOLT_PRICING_FILE`.
      Confirm unknown models fail before upstream.
- [ ] Require a positive output maximum on every non-streaming request or set a
      deliberately sized default.
- [ ] Confirm the selected request/output fields are in the documented
      text/refusal/function/tool/structured accounting surface.
- [ ] Choose request, upstream-body, SSE-frame, and upstream-header timeout
      bounds for the workload.
- [ ] Decide explicitly whether company telemetry remains disabled.
- [ ] Keep provider-side budget alerts and limits enabled.
- [ ] Leave the embedded mock disabled.

## Persistence and scaling

- [ ] Accept that a restart resets all financial/token state, or do not deploy
      this phase where restart persistence is required.
- [ ] Run one Kilovolt instance per budget domain. Multiple replicas have
      independent ledgers.
- [ ] Ensure Docker/Compose/Kubernetes replica count is exactly one and rolling
      deployments do not briefly overlap processes for one logical budget.
- [ ] Plan external request rate/connection limits; Kilovolt does not provide
      them.
- [ ] Protect and retain logs according to the sensitivity of user IDs and
      usage metadata.

## Validation

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release
cp .env.example .env
# Replace both blank secrets in .env before continuing.
docker compose config --quiet
docker compose up -d
curl --fail http://127.0.0.1:8080/health
docker compose down --remove-orphans
docker compose -f docker-compose.demo.yml config --quiet
docker compose -f docker-compose.demo.yml up -d --build
scripts/demo-smoke.sh
docker compose -f docker-compose.demo.yml down --remove-orphans
scripts/run-race-tests.sh 100
python3 scripts/check-docs.py
scripts/smoke-cookbook.sh
python3 scripts/telemetry-smoke.py
python3 scripts/benchmark/run.py --smoke
docker build -t kilovolt:local .
```

After starting a container, check:

```bash
docker inspect --format '{{.State.Health.Status}}' kilovolt
curl --fail http://127.0.0.1:8080/health
curl --user 'kilovolt:<dashboard-token-from-.env>' \
  http://127.0.0.1:8080/api/stats
```

Also verify that the same dashboard request without credentials returns `401`,
that proxy requests without `X-Kilovolt-Key` return `401`, that the mock route
returns `404` by default, and use a local receiver to confirm both
telemetry-disabled and explicitly enabled behavior.

## Operational alerts

- [ ] Alert on local `429`, `499`, `502`, and `504` request records.
- [ ] Alert on process restarts because they reset the ledger.
- [ ] Alert on `provider_usage_exceeded_reserved_bound` and other conservative
      full-reservation finalizations; compare against provider records.
- [ ] Alert on provider-side spend independently from Kilovolt.
- [ ] Track RSS after tokenizer warm-up and under the intended concurrency.
- [ ] Re-run benchmarks and price validation after model, tokenizer, compiler,
      or deployment-host changes.
