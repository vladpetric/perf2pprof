#!/usr/bin/env bash
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
work=$(mktemp -d /tmp/p2prof-zstd-test.XXXXXX)
out="$work/artifacts"

cleanup() {
  if [[ ${KEEP_ARTIFACTS:-0} == 1 ]]; then
    printf 'Artifacts retained: %s\n' "$out"
  else
    rm -rf -- "$work"
  fi
}
trap cleanup EXIT

for tool in cargo perf zstd; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    printf 'SKIP: required command not found: %s\n' "$tool" >&2
    exit 77
  fi
done

if ! perf record -q -e cycles -o "$work/perf-probe.data" -- true; then
  printf 'SKIP: perf recording is not permitted in this environment\n' >&2
  exit 77
fi

cargo build --quiet --release --manifest-path "$root/Cargo.toml" --bins
P2PROF="$root/target/release/p2prof" \
  "$root/scripts/zstd-profile-benchmark.sh" \
  --size-mib "${SIZE_MIB:-32}" \
  --frequency "${FREQUENCY:-199}" \
  --out "$out"

events=(cycles instructions branch-misses L1-dcache-load-misses)
workloads=(compress decompress)
symbolized=0

for workload in "${workloads[@]}"; do
  for event in "${events[@]}"; do
    report="$out/$workload.$event.top.txt"
    graph="$out/$workload.$event.png"
    if [[ ! -s $report || ! -s $graph ]]; then
      printf 'FAIL: missing report or graph for %s %s\n' "$workload" "$event" >&2
      exit 1
    fi
    if ! grep -q '^Type: ' "$report" || ! grep -Eq '[[:space:]][0-9]+(\.[0-9]+)?%[[:space:]]' "$report"; then
      printf 'FAIL: malformed report: %s\n' "$report" >&2
      exit 1
    fi
    if grep -Eq '[[:space:]]ZSTD_[[:alnum:]_.$]+' "$report"; then
      symbolized=1
    fi
  done
done

if (( symbolized )); then
  printf 'PASS: reports and graphs validated with zstd symbols\n'
else
  printf 'PASS: reports and graphs validated without requiring zstd symbols\n'
fi
