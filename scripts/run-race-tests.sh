#!/usr/bin/env bash
set -euo pipefail

iterations="${1:-100}"

if ! [[ "$iterations" =~ ^[1-9][0-9]*$ ]]; then
  echo "usage: $0 [positive-iteration-count]" >&2
  exit 2
fi

for ((iteration = 1; iteration <= iterations; iteration++)); do
  echo "race-test iteration ${iteration}/${iterations}"
  cargo test concurrent_ --all-targets --all-features -- --test-threads=1
done
