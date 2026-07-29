# Reproducible benchmarks

Kilovolt ships a standard-library Python harness at
`scripts/benchmark/run.py`. It launches the release binary, uses the embedded
deterministic mock as both a direct baseline and proxied upstream, runs warm-up
and measured requests, samples process RSS/CPU with `ps`, and writes raw JSON.

## Commands

```bash
cargo build --release
python3 scripts/benchmark/run.py --smoke
python3 scripts/benchmark/run.py
```

The full run covers streaming and non-streaming at concurrency 1, 10, 50, and
100; a 5,000-event stream; a 512 KiB prompt; preflight rejection; and
mid-stream cutoff. Results default to `benchmark-results/<UTC timestamp>.json`.

## Verified local run

Raw evidence:

- [`20260729-macos-arm64.json`](../benchmark-results/20260729-macos-arm64.json)
- [`20260729-macos-arm64-run2.json`](../benchmark-results/20260729-macos-arm64-run2.json)
- [`20260729-macos-arm64-run3.json`](../benchmark-results/20260729-macos-arm64-run3.json)
- [`20260729-macos-arm64-pre-tokenizer-fix.json`](../benchmark-results/20260729-macos-arm64-pre-tokenizer-fix.json)
- [`smoke.json`](../benchmark-results/smoke.json)

Environment: 2026-07-29, Apple M4 (10 logical CPUs), 16 GiB RAM, macOS
26.5.1 arm64, Rust recorded in the raw file, release profile, repository HEAD
`88aefa4` with a dirty working tree. Because the Phase 3 changes were
uncommitted, the commit alone does not reproduce the exact source; the raw file
explicitly records `working_tree_dirty: true`.

Ranges across three post-fix full runs:

| Scenario | Direct p50 range | Proxied p50 range | Paired difference range | Observed peak RSS range |
|---|---:|---:|---:|---:|
| Streaming, concurrency 1, TTFB | 0.203–0.283 ms | 0.176–0.265 ms | -0.034–+0.062 ms | 61.5–61.6 MiB |
| Streaming, concurrency 100, TTFB | 2.977–3.186 ms | 2.743–3.528 ms | -0.442–+0.490 ms | 64.1–64.5 MiB |
| JSON, concurrency 1, total | 0.107–0.115 ms | 0.149–0.152 ms | +0.036–+0.042 ms | 64.6 MiB |
| 5,000-event stream, total | 2.484–2.663 ms | 21.233–21.371 ms | +18.572–+18.772 ms | 64.7–65.4 MiB |
| 512 KiB JSON prompt, total | 0.225–0.272 ms | 24.043–26.977 ms | +23.819–+26.720 ms | 93.4–93.6 MiB |
| Preflight rejection, concurrency 10 | n/a | 0.982–1.062 ms; 6,714–7,452 req/s | n/a | 60.3–60.5 MiB |
| Mid-stream cutoff | n/a | 0.324–0.396 ms | n/a | 61.4–61.5 MiB |

Idle RSS before tokenizer initialization ranged from 10,208–10,256 KiB. Normal
streaming initialized the shared tokenizer and warmed RSS reached about
61.5–64.5 MiB across the ordered runs. The proxied long-stream peak was between
0 and 208 KiB above the immediately preceding direct-long-stream high-water
mark, so these runs did not show memory proportional to 5,000 event frames. The
large prompt temporarily raised RSS to 95,632–95,888 KiB.

The pre-fix evidence records a benchmark-discovered bug: cloning the tokenizer
per stream produced a 1,094,480 KiB peak at concurrency 100. Reusing the
tokenizer's static shared instance reduced the comparable post-fix
concurrency-100 peak to 65,648–66,048 KiB.

## Interpretation limits

- Direct and proxied traffic share one process and loopback interface. This
  isolates the code path but is not a provider/network benchmark.
- Very fast local workloads are sensitive to ordering, scheduling, cache
  warm-up, and the coarse `ps` CPU sampler. Most short scenarios contain only
  one process sample. Several paired p50 differences are negative and must be
  treated as measurement noise.
- CPU samples are process percentages reported by `ps`, not hardware-counter
  profiles.
- A mid-stream cutoff retains the already-sent HTTP `200` status; the local
  request record is `429`.
- No Go or Python gateway was tested. This repository makes no comparative
  cross-language performance claim.
- Run several repetitions on the intended host and report ranges/percentiles
  rather than selecting a best result.

## Manual workflow

`.github/workflows/benchmark.yml` provides a manually dispatched full or smoke
run and uploads the JSON artifact. Load benchmarks are intentionally excluded
from normal CI.
