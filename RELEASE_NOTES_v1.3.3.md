# Kilovolt v1.3.3 — Guided Local Evaluation

Kilovolt v1.3.3 adds a one-command, browser-guided evaluation experience while preserving the financial-safety behavior introduced in v1.3.2.

## Highlights

### One-command evaluation flow

Running the default Docker image with no manual configuration now opens a local setup journey at `http://127.0.0.1:8080`:

1. Configure an OpenAI API key and initial spending limits.
2. Verify the full authentication, budget, provider, and accounting path with one small capped request.
3. Connect a trusted backend with Python, JavaScript, or curl.
4. Monitor project and user spend from the local dashboard.

The Configure, Verify, and Connect steps are now real navigation controls. Completed steps can be revisited, browser Back follows the stage history, and Connect stays locked until a verification request succeeds.

### Local integration documentation

The evaluation UI now includes self-contained Python, JavaScript, and curl examples. GitHub documentation remains available as a deeper reference instead of being required for the first successful integration.

### Editable project and user limits

Evaluation-mode operators can update both the project-wide budget and the default per-user budget after setup. Changes apply immediately and preserve accumulated spend; lowering a limit below current spend blocks future requests until the limit is raised.

`KILOVOLT_DEFAULT_BUDGET` remains the default per-user budget. Project-wide limits remain separately enforced.

### Safer secret handling

- The OpenAI provider key is masked after setup and is never returned to the browser in full.
- The Kilovolt gateway key is masked by default on the Connect step.
- `X-User-ID` remains required and must be inserted or overwritten by a trusted backend after user authentication.
- Untrusted browser or mobile clients must not be allowed to choose arbitrary `X-User-ID` values.

## Upgrade notes

No configuration migration is required from v1.3.2. Existing manually configured deployments continue to use their current environment-variable flow.

The guided setup is intentionally temporary in this phase:

- Evaluation configuration, keys, calculated spend, and verification state are stored in memory.
- Restarting the container resets the evaluation journey.
- A single process represents one implicit application project.
- Kilovolt remains a financial circuit breaker, not a replacement for provider billing records or provider-side limits.

## Docker release

Publish the GitHub release with the exact tag `v1.3.3`. The Docker workflow verifies that the release tag matches `Cargo.toml` before publishing:

- `yodsarun/kilovolt-proxy:latest`
- `yodsarun/kilovolt-proxy:v1.3.3`
