#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/test-user-guide-examples.sh [case]

Cases:
  all                 Run every case (default)
  frequency-fp        Fixed-frequency cycles with frame-pointer call graphs
  frequency-dwarf     Fixed-frequency cycles with DWARF call graphs
  counter-period      Fixed-period retired-instruction sampling with DWARF
  multi-counter       Instructions, branch misses, and L1 misses with DWARF

Environment:
  SIZE_MIB=N          Input size in MiB (default: 32)
  FREQUENCY=N         Samples per second for frequency cases (default: 99)
  FREQUENCY_EVENT=E   Event for frequency cases (default: cycles)
  COUNTER_EVENT=E     Event for the fixed-period case (default: instructions)
  COUNTER_PERIOD=N    Events per sample (default: 10000000)
  MULTI_COUNTER_EVENTS="E ..."  Events for the multi-counter case
  KEEP_ARTIFACTS=1    Retain the temporary output directory
  PERF, P2PROF, ZSTD  Override the corresponding commands
EOF
}

case_name=${1:-all}
if [[ $# -gt 1 ]]; then
  usage >&2
  exit 2
fi
case "$case_name" in
  all|frequency-fp|frequency-dwarf|counter-period|multi-counter) ;;
  -h|--help)
    usage
    exit 0
    ;;
  *)
    printf 'unknown test case: %s\n' "$case_name" >&2
    usage >&2
    exit 2
    ;;
esac

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
work=$(mktemp -d /tmp/p2prof-guide-test.XXXXXX)
out="$work/artifacts"
mkdir -p -- "$out"

cleanup() {
  if [[ ${KEEP_ARTIFACTS:-0} == 1 ]]; then
    printf 'Artifacts retained: %s\n' "$out"
  else
    rm -rf -- "$work"
  fi
}
trap cleanup EXIT

size_mib=${SIZE_MIB:-32}
frequency=${FREQUENCY:-99}
frequency_event=${FREQUENCY_EVENT:-cycles}
counter_event=${COUNTER_EVENT:-instructions}
counter_period=${COUNTER_PERIOD:-${INSTRUCTION_PERIOD:-10000000}}
multi_counter_events=${MULTI_COUNTER_EVENTS:-instructions branch-misses L1-dcache-load-misses}
for value in "$size_mib" "$frequency" "$counter_period"; do
  if [[ ! $value =~ ^[1-9][0-9]*$ ]]; then
    printf 'SIZE_MIB, FREQUENCY, and COUNTER_PERIOD must be positive integers\n' >&2
    exit 2
  fi
done

perf=${PERF:-perf}
zstd=${ZSTD:-zstd}
for tool in cargo cmp dot "$perf" sha256sum "$zstd"; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    printf 'SKIP: required command not found: %s\n' "$tool" >&2
    exit 77
  fi
done

if ! "$perf" record -q -e "$frequency_event" -o "$work/perf-probe.data" -- true; then
  printf 'SKIP: perf recording is not permitted in this environment\n' >&2
  exit 77
fi

cargo build --quiet --release --manifest-path "$root/Cargo.toml" --bins --examples
p2prof=${P2PROF:-$root/target/release/p2prof}
zstd=$(command -v "$zstd")
generator="$root/target/release/examples/seeded_text"
input="$out/input.txt"
minimum_bytes=$(( size_mib * 1024 * 1024 ))

"$generator" --bytes "$minimum_bytes" >"$input" 2>"$out/generator.log"
sha256sum "$input" >"$out/input.sha256"

validate_output() {
  local report=$1
  local graph=$2
  if [[ ! -s $report || ! -s $graph ]]; then
    printf 'FAIL: missing report or graph: %s %s\n' "$report" "$graph" >&2
    exit 1
  fi
  if ! grep -q '^Type: ' "$report" \
    || ! grep -Eq '[[:space:]][0-9]+(\.[0-9]+)?%[[:space:]]' "$report"; then
    printf 'FAIL: malformed report: %s\n' "$report" >&2
    exit 1
  fi
}

render_event() {
  local test_name=$1
  local workload=$2
  local event=$3
  local data=$4
  local prefix="$out/$test_name.$workload.$event"
  "$p2prof" --event "$event" -top -nodecount=10 \
    "$zstd" "$data" >"$prefix.top.txt"
  "$p2prof" --event "$event" -png -nodecount=80 \
    -output "$prefix.png" "$zstd" "$data"
  validate_output "$prefix.top.txt" "$prefix.png"
}

run_case() {
  local test_name=$1
  local event_list=$2
  shift 2
  local record=("$@")
  local archive="$out/$test_name.zst"
  local compress_data="$out/$test_name.compress.perf.data"
  local decompress_data="$out/$test_name.decompress.perf.data"
  local event

  printf 'Running %s compression...\n' "$test_name"
  cat "$input" | "${record[@]}" -o "$compress_data" -- \
    "$zstd" -q -T1 -3 -o "$archive"
  "$zstd" -q -T1 -dc "$archive" | cmp "$input" -

  printf 'Running %s decompression...\n' "$test_name"
  "${record[@]}" -o "$decompress_data" -- \
    "$zstd" -q -T1 -dc "$archive" | cmp "$input" -

  for event in $event_list; do
    render_event "$test_name" compress "$event" "$compress_data"
    render_event "$test_name" decompress "$event" "$decompress_data"
  done
  printf 'PASS: %s\n' "$test_name"
}

run_frequency_fp() {
  run_case frequency-fp "$frequency_event" \
    "$perf" record -F "$frequency" -e "$frequency_event" -g --call-graph fp
}

run_frequency_dwarf() {
  run_case frequency-dwarf "$frequency_event" \
    "$perf" record -F "$frequency" -e "$frequency_event" -g --call-graph dwarf,4096
}

run_counter_period() {
  run_case counter-period "$counter_event" \
    "$perf" record -c "$counter_period" -e "$counter_event" \
    -g --call-graph dwarf,4096
}

run_multi_counter() {
  local events=()
  local event
  for event in $multi_counter_events; do
    events+=( -e "$event" )
  done
  run_case multi-counter "$multi_counter_events" \
    "$perf" record -F "$frequency" -g --call-graph dwarf,4096 "${events[@]}"
}

if [[ $case_name == all ]]; then
  run_frequency_fp
  run_frequency_dwarf
  run_counter_period
  run_multi_counter
else
  "run_${case_name//-/_}"
fi

printf 'All requested user-guide example tests passed.\n'
