# Production checklist

## Before deployment

- [ ] Read [security.md](security.md) and [architecture.md](architecture.md).
- [ ] Put Kilovolt behind the authenticated founder backend.
- [ ] Strip any client-supplied `X-User-ID`, then insert the authenticated user
      identifier server-side.
- [ ] Bind privately or place a TLS reverse proxy/firewall in front.
- [ ] Generate `KILOVOLT_DASHBOARD_TOKEN` with at least 32 random bytes.
- [ ] Set a project budget and a per-user default budget.
- [ ] Validate all model prices in `src/budget.rs` against the provider.
- [ ] Confirm the selected model/request fields are covered by Kilovolt's token
      accounting; tool/function payload accounting remains limited.
- [ ] Choose request, upstream-body, SSE-frame, and upstream-header timeout
      bounds for the workload.
- [ ] Decide explicitly whether company telemetry remains disabled.
- [ ] Keep provider-side budget alerts and limits enabled.

## Persistence and scaling

- [ ] Accept that a restart resets all financial/token state, or do not deploy
      this phase where restart persistence is required.
- [ ] Run one Kilovolt instance per budget domain. Multiple replicas have
      independent ledgers.
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
scripts/run-race-tests.sh 100
python3 scripts/check-docs.py
scripts/smoke-cookbook.sh
python3 scripts/benchmark/run.py --smoke
docker build -t kilovolt:local .
```

After starting a container, check:

```bash
docker inspect --format '{{.State.Health.Status}}' kilovolt-test
curl --fail http://127.0.0.1:8080/health
curl --user "kilovolt:${KILOVOLT_DASHBOARD_TOKEN}" \
  http://127.0.0.1:8080/api/stats
```

Also verify that the same dashboard request without credentials returns `401`,
and use a local receiver to confirm both telemetry-disabled and explicitly
enabled behavior.

## Operational alerts

- [ ] Alert on local `429`, `499`, `502`, and `504` request records.
- [ ] Alert on process restarts because they reset the ledger.
- [ ] Alert on provider-side spend independently from Kilovolt.
- [ ] Track RSS after tokenizer warm-up and under the intended concurrency.
- [ ] Re-run benchmarks and price validation after model, tokenizer, compiler,
      or deployment-host changes.
