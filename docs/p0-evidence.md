# P0 evaluation evidence — v1.3.5

Evaluation date: 2026-08-29 (Asia/Bangkok)

## Reproduce

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release
docker compose -f docker-compose.demo.yml up -d --build
./scripts/demo-smoke.sh
docker compose -f docker-compose.demo.yml down --remove-orphans
```

The Docker smoke script expects `PASS` for health, wrong gateway credential,
unknown model pricing, missing identity, accepted bounded request, user-budget
rejection, project-budget rejection, streaming cutoff, authenticated stats, and
dashboard rendering. It fails if a missing identity creates an anonymous spending
ledger.

## Pricing provenance

Model: `gpt-4o-mini-2024-07-18`

Verified price: $0.15 / 1M input tokens and $0.60 / 1M output tokens.

Official source:
<https://developers.openai.com/api/docs/models/gpt-4o-mini>

Verified on: 2026-08-29. The official page does not state a separate price
effective date, so no unsupported effective-date claim is made.

## Results

The P0 validation run completed with these results:

- `cargo fmt --check`: passed.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.
- `cargo test --all-targets --all-features`: 107 passed, 0 failed.
- `cargo build --release`: passed for Kilovolt 1.3.5.
- `python3 scripts/check-docs.py`: 64 local links checked successfully.
- Next.js production build: passed.
- Provider-free Docker smoke: every authentication, pricing, identity, accepted
  request, user/project rejection, streaming cutoff, statistics, and dashboard
  assertion passed.
- Real OpenAI verification: completed by the project owner through the bounded
  dashboard test; the key remained masked and was not stored in the evidence.
- Credential scan: key-like strings are limited to deliberately fake CI/test
  fixtures in `.github/workflows/ci.yml` and `src/dashboard.rs`.

The validated source is committed on `codex/p0-final-mvp-closure`. The immutable
release reference will be the `v1.3.5` tag after review and merge; the Docker
release workflow rejects any tag that does not match Cargo metadata.
