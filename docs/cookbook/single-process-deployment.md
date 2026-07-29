# Single-process deployment

## Goal

Deploy one process for one logical project budget while acknowledging that its
ledger is memory-only.

## Request flow

```text
one logical budget -> exactly one Kilovolt process -> one in-memory ledger
```

## Prerequisites

An operational design that tolerates spend reset on restart and does not run
overlapping replicas.

## Complete configuration

```bash
export BIND_ADDR=0.0.0.0
export KILOVOLT_ACKNOWLEDGE_PROCESS_LOCAL_LEDGER=true
export KILOVOLT_PROXY_TOKEN="$(openssl rand -hex 32)"
```

Loopback development does not require the acknowledgement.

## Complete runnable code

One host:

```bash
./target/release/kilovolt
```

Kubernetes must state one replica:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata: {name: kilovolt}
spec:
  replicas: 1
  strategy: {type: Recreate}
  selector: {matchLabels: {app: kilovolt}}
  template:
    metadata: {labels: {app: kilovolt}}
    spec:
      containers:
        - name: kilovolt
          image: kilovolt:audited
          env:
            - {name: BIND_ADDR, value: "0.0.0.0"}
            - {name: KILOVOLT_ACKNOWLEDGE_PROCESS_LOCAL_LEDGER, value: "true"}
            - name: KILOVOLT_PROXY_TOKEN
              valueFrom: {secretKeyRef: {name: kilovolt, key: proxy-token}}
```

`Recreate` avoids normal rolling overlap, but restart still resets spend.

## Verify it works

Non-loopback startup without acknowledgement must fail. Authenticated
`/api/stats` must report memory/process scope, restart reset `true`, and
multi-instance safety `false`.

## Expected success behavior

One live process enforces one project ledger and per-user ledgers until it
stops.

## Expected budget-block behavior

Atomic limits apply only inside that process. Starting a second process creates
another full budget and can multiply provider exposure.

## Security notes

The acknowledgement only prevents accidental configuration; it adds no storage
or coordination. Keep provider-side limits and alerts as defense in depth.
Planned maintenance/restarts must account for reset spend.

## Common failure modes

Rolling deployments, autoscaling, crash restarts, and multiple hosts create
fresh independent ledgers. Use `replicas: 1`, avoid overlap, and do not claim
persistence.
