#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/zstd-profile-benchmark.sh [options]

Options:
  --size-mib N    Uncompressed input size in MiB (default: 400)
  --frequency N   Sampling frequency per event (default: 99)
  --level N       zstd compression level (default: 3)
  --out DIR       New output directory (default: /tmp/p2prof-zstd-<UTC time>)
  -h, --help      Show this help

Environment overrides: PERF, P2PROF, ZSTD, FREQUENCY_EVENT, MICROARCH_EVENTS
EOF
}

size_mib=400
frequency=99
level=3
out=

while (( $# )); do
  case "$1" in
    --size-mib)
      size_mib=${2:?missing value for --size-mib}
      shift 2
      ;;
    --frequency)
      frequency=${2:?missing value for --frequency}
      shift 2
      ;;
    --level)
      level=${2:?missing value for --level}
      shift 2
      ;;
    --out)
      out=${2:?missing value for --out}
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

for value in "$size_mib" "$frequency" "$level"; do
  if [[ ! $value =~ ^[1-9][0-9]*$ ]]; then
    printf 'size, frequency, and level must be positive integers\n' >&2
    exit 2
  fi
done

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
perf=${PERF:-perf}
p2prof=${P2PROF:-p2prof}
zstd=${ZSTD:-zstd}
frequency_event=${FREQUENCY_EVENT:-cycles}
microarch_events=${MICROARCH_EVENTS:-instructions branch-misses L1-dcache-load-misses}

for tool in cargo cmp dot "$perf" "$p2prof" sha256sum "$zstd"; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    printf 'required command not found: %s\n' "$tool" >&2
    exit 1
  fi
done

if [[ -z $out ]]; then
  out="/tmp/p2prof-zstd-$(date -u +%Y%m%d-%H%M%S)"
fi
if [[ -e $out ]]; then
  printf 'output path already exists: %s\n' "$out" >&2
  exit 1
fi
mkdir -p -- "$out"
out=$(cd -- "$out" && pwd)

zstd=$(command -v "$zstd")
p2prof=$(command -v "$p2prof")
perf=$(command -v "$perf")
generator="$root/target/release/examples/seeded_text"
input="$out/input.txt"
frequency_archive="$out/input.frequency.zst"
microarch_archive="$out/input.microarch.zst"
minimum_bytes=$(( size_mib * 1024 * 1024 ))

has_build_id_debug_file() {
  local binary=$1
  local build_id
  if ! command -v readelf >/dev/null 2>&1; then
    return 1
  fi
  build_id=$(readelf -n "$binary" 2>/dev/null | awk '/Build ID:/ { print $3; exit }')
  [[ -n $build_id ]] || return 1
  [[ -r /usr/lib/debug/.build-id/${build_id:0:2}/${build_id:2}.debug ]]
}

if command -v file >/dev/null 2>&1 \
  && file "$zstd" | grep -q 'stripped' \
  && ! has_build_id_debug_file "$zstd"; then
  printf 'Warning: %s is stripped; install its matching debug symbols for named-function reports.\n' "$zstd" >&2
fi

printf 'Building deterministic text generator...\n'
cargo build --quiet --release --manifest-path "$root/Cargo.toml" --example seeded_text
"$generator" --bytes "$minimum_bytes" >"$input" 2>"$out/generator.log"
sha256sum "$input" >"$out/input.sha256"

cat >"$out/parameters.txt" <<EOF
size_mib=$size_mib
minimum_bytes=$minimum_bytes
frequency=$frequency
zstd_level=$level
zstd=$zstd
p2prof=$p2prof
perf=$perf
frequency_events=$frequency_event
microarchitecture_events=${microarch_events// /,}
EOF

frequency_record=("$perf" record -F "$frequency" -g --call-graph dwarf,4096 -e "$frequency_event")
microarch_record=("$perf" record -F "$frequency" -g --call-graph dwarf,4096)
for event in $microarch_events; do
  microarch_record+=( -e "$event" )
done

printf 'Profiling single-threaded compression (%s)...\n' "$frequency_event"
cat "$input" | "${frequency_record[@]}" \
  -o "$out/compress.frequency.perf.data" -- \
  "$zstd" -q -T1 "-$level" -o "$frequency_archive"
"$zstd" -q -T1 -dc "$frequency_archive" | cmp "$input" -

printf 'Profiling single-threaded compression (three PMU events)...\n'
cat "$input" | "${microarch_record[@]}" \
  -o "$out/compress.microarch.perf.data" -- \
  "$zstd" -q -T1 "-$level" -o "$microarch_archive"
"$zstd" -q -T1 -dc "$microarch_archive" | cmp "$input" -

printf 'Profiling verified single-threaded decompression (%s)...\n' "$frequency_event"
"${frequency_record[@]}" -o "$out/decompress.frequency.perf.data" -- \
  "$zstd" -q -T1 -dc "$frequency_archive" | cmp "$input" -

printf 'Profiling verified single-threaded decompression (three PMU events)...\n'
"${microarch_record[@]}" -o "$out/decompress.microarch.perf.data" -- \
  "$zstd" -q -T1 -dc "$frequency_archive" | cmp "$input" -

render() {
  local workload=$1
  local data=$2
  shift 2
  local event
  for event in "$@"; do
    "$p2prof" --event "$event" -top -nodecount=10 \
      "$zstd" "$data" >"$out/$workload.$event.top.txt"
    "$p2prof" --event "$event" -png -nodecount=80 \
      -output "$out/$workload.$event.png" "$zstd" "$data"
  done
}

printf 'Rendering pprof reports...\n'
render compress "$out/compress.frequency.perf.data" "$frequency_event"
render compress "$out/compress.microarch.perf.data" $microarch_events
render decompress "$out/decompress.frequency.perf.data" "$frequency_event"
render decompress "$out/decompress.microarch.perf.data" $microarch_events

printf 'All decompression comparisons passed.\n'
printf 'Artifacts: %s\n' "$out"
