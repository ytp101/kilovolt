# Kilovolt v1.3.4 — Faster One-Command Evaluation

Kilovolt v1.3.4 closes the P0 evaluation UX pass and publishes the onboarding and dashboard improvements that landed after v1.3.3.

## Highlights

### Clear one-command entry

- The landing page and README lead with the loopback-only Docker command.
- Startup output prints a compact readiness block with a clickable local dashboard URL.
- No Git checkout, Cargo build, Docker Compose file, or `.env` file is required to begin evaluating Kilovolt.

### Simpler setup and optional verification

- OpenAI key management is linked directly from the provider-key field.
- Spending limits are optional, with clear project and default per-user values.
- The paid verification request is recommended but skippable.
- Verification results show model output, token counts, calculated spend, remaining project budget, and latency.

### Connect from the dashboard

- The dashboard opens its Connect your application panel during onboarding.
- The gateway key is masked by default and can be copied or revealed.
- Copyable `.env`, Python, TypeScript, and curl examples are available in one place.
- The recommended Python flow uses `python-dotenv` and identifies `local-test-user` as evaluation-only.
- The `X-User-ID` guidance explains that a trusted backend must set or overwrite the authenticated user identity.

### Faster financial feedback

- Project spend, default user limit, accepted requests, and blocked requests are prioritized above system health.
- Recent transactions use human-readable results and update through the existing refresh flow.
- Code blocks, keys, tabs, and dashboard cards remain usable on narrow screens.

## Upgrade notes

No configuration migration is required from v1.3.3. Existing manually configured deployments keep their current environment-variable behavior.

Evaluation state remains intentionally in memory and resets when the process restarts. Kilovolt still models one implicit application project in the current self-hosted phase.

## Docker release

The GitHub release tag must be exactly `v1.3.4`. The release workflow verifies that the tag matches `Cargo.toml` before publishing:

- `yodsarun/kilovolt-proxy:latest`
- `yodsarun/kilovolt-proxy:v1.3.4`
