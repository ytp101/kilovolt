# Kilovolt academic project statement

## Ownership and evaluated release

Kilovolt is an academic project owned by the GitHub account `ytp101`. The final
P0 evaluation target is release `v1.3.5`. The repository is all-rights-reserved;
public visibility does not grant permission to reuse the work. See [LICENSE](LICENSE).

## Problem

An application owner calling an AI API needs an enforceable local guardrail for
calculated spend. Provider invoices arrive outside the request path, while an
application may need to reject a request or stop a stream as soon as its project
or end-user allowance is exhausted.

## Target user

The target user is a founder or application owner operating a trusted backend
between authenticated end users and the OpenAI Chat Completions API. The backend
must derive and overwrite `X-User-ID`; Kilovolt does not authenticate end users.

## P0 scope

The evaluated MVP is a self-hosted, single-process Rust gateway supporting the
OpenAI Chat Completions route. It estimates prompt and supported output cost,
atomically checks one implicit-project budget and one default per-user budget,
proxies streaming and bounded non-streaming responses, and presents local
calculated-spend evidence in a dashboard. The pinned demonstration model is
`gpt-4o-mini-2024-07-18`.

Custom upstream URLs remain an advanced, unverified compatibility setting. Other
providers, OpenAI API families, hosted multi-tenancy, persistence, individual
per-user budget administration, and exact invoice reconciliation are outside P0.

## Architecture

```text
authenticated end user
  -> trusted application backend inserts X-User-ID
  -> Kilovolt authentication and identity validation
  -> exact model-price lookup
  -> atomic project + user reservation
  -> OpenAI Chat Completions
  -> atomic project + user output charge
  -> local dashboard and transaction history
```

Both financial ledgers are guarded by one lock. A failed project or user check
leaves both unchanged. Streaming output is charged before each supported frame is
forwarded; bounded non-streaming requests reserve maximum output and settle after
the complete response is validated.

## Limitations

- Setup, ledgers, and request history exist only in process memory and reset on
  restart.
- One process represents one implicit application project; replicas do not share
  state.
- Every trusted user receives the same configured default limit.
- Dollar calculations use floating-point values and local/provider token fields;
  they may differ from an OpenAI invoice.
- Kilovolt is an application guardrail, not provider-side billing authority,
  authentication, a secret manager, or a production distributed gateway.
- The recorded OpenAI price has a verification date, not a guaranteed future
  validity period. Provider-side budgets and alerts remain necessary.

## Evaluation method

The release is evaluated with Rust formatting, warning-free Clippy, all-target
tests, and a release build. A provider-free Docker Compose journey proves health,
authentication, exact-price fail-closed behavior, required identity, an accepted
request, user/project rejection, streaming cutoff, nonzero ledgers, and dashboard
rendering. A separately authorized real OpenAI request uses a 16-token output cap
and is checked for provider-secret disclosure. Commands and outcomes are recorded
in [docs/p0-evidence.md](docs/p0-evidence.md).
