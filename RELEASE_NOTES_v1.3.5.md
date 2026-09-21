# Kilovolt v1.3.5 — Final Academic MVP Closure

Kilovolt v1.3.5 narrows and verifies the final academic MVP boundary.

## Supported scope

- Official support is limited to the OpenAI Chat Completions route.
- The demonstration is pinned to `gpt-4o-mini-2024-07-18`.
- The legacy Gemini translation and bundled Gemini pricing paths are removed.
- Custom upstream URLs remain available only as unverified compatibility behavior.

## Fail-closed accounting identity

- Every accepted proxy request requires a valid trusted `X-User-ID`.
- Missing or malformed identities return an OpenAI-shaped `400 invalid_user_id`
  before body parsing, upstream contact, or financial-ledger mutation.
- The application backend must authenticate the end user and overwrite this
  header; untrusted browser/mobile clients must not select it.

## Reproducible pricing

- The built-in demonstration price is $0.15 per million input tokens and $0.60
  per million output tokens.
- Values were verified on 2026-08-29 against the official OpenAI GPT-4o Mini
  model page. That page does not provide a separate price effective date.
- Exact identifiers are required; unsupported model variants fail before
  upstream contact and ledger changes.

## Academic metadata

- The project is explicitly all-rights-reserved and is not presented as open
  source.
- The problem, target user, architecture, scope, limitations, and evaluation
  method are recorded in `ACADEMIC_PROJECT.md`.
- Evidence and remaining manual validation are recorded in
  `docs/p0-evidence.md`.

## Release alignment

The release tag must be exactly `v1.3.5`. The Docker workflow verifies that tag
against Cargo metadata before publishing:

- `yodsarun/kilovolt-proxy:latest`
- `yodsarun/kilovolt-proxy:v1.3.5`
