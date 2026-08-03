# p2prof and perf2pprof

`p2prof` is the normal entry point for viewing Linux `perf.data` files with
Google pprof. It accepts perf data directly, converts it temporarily with
`perf2pprof`, and preserves the familiar pprof command-line interface:

```sh
perf record -F 99 -e cycles -g --call-graph dwarf,4096 \
  -o profile.perf.data -- ./path/to/binary arg...
p2prof --event cycles -top ./path/to/binary profile.perf.data
```

See [USER_GUIDE.md](USER_GUIDE.md) for modern pprof installation, extensive
`p2prof` examples, web and static graph usage, and the reproducible zstd
profiling benchmark.

## Prerequisites

- Linux and a `perf` package matching the running kernel
- Rust and Cargo to install `p2prof` and `perf2pprof`
- a current Go toolchain to install Google pprof
- a current upstream Google pprof, installed with Go rather than an old distro
  `pprof`, `google-pprof`, or gperftools script
- Graphviz `dot` for PNG, SVG, PDF, and interactive graph views
- debug symbols when named internal functions are required

## Installation

Install a current upstream Google pprof first. Old distro pprof variants are
not supported:

```sh
go install github.com/google/pprof@latest
export PATH="$(go env GOPATH)/bin:$PATH"
```

Install directly from GitHub with Cargo:

```sh
cargo install --git https://github.com/vladpetric/perf2pprof
```

Cargo installs both `perf2pprof` and `p2prof` into `~/.cargo/bin` by default.
Make sure that
directory is on your `PATH`:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

Alternatively, build a release binary manually:

```sh
git clone https://github.com/vladpetric/perf2pprof
cd perf2pprof
cargo build --release
install -D -m 0755 target/release/perf2pprof ~/.local/bin/perf2pprof
install -D -m 0755 target/release/p2prof ~/.local/bin/p2prof
```

## Collecting Perf Data

The normal workflow is only `perf record` followed by `p2prof`. For a program
built with frame pointers:

```sh
perf record -F 99 -e cycles -g --call-graph fp \
  -o profile.perf.data -- ./path/to/binary arg...
p2prof --event cycles -top ./path/to/binary profile.perf.data
```

Use DWARF call graphs when the binary was built without frame pointers or when
you want to compare profile data without changing compiler flags:

```sh
perf record -F 49 -e cycles -g --call-graph dwarf,4096 \
  -o profile.perf.data -- ./path/to/binary arg...
p2prof --event cycles -top ./path/to/binary profile.perf.data
```

Sample low-level PMU counters together and select one when rendering:

```sh
perf record -F 99 -g --call-graph dwarf,4096 \
  -e instructions -e branch-misses -e L1-dcache-load-misses \
  -o counters.perf.data -- ./path/to/binary arg...
p2prof --event branch-misses -top ./path/to/binary counters.perf.data
```

See the user guide for fixed counter-period collection, graphs, and web UI
examples.

The `-F` option controls sampling frequency, not whether an event is a hardware
counter. `-F 99 -e instructions` still uses the hardware retired-instructions
counter; the kernel adjusts its overflow period to target 99 samples per
second. Use `-c` for a fixed raw-event period instead:

```sh
perf record -c 10000000 -e instructions -g --call-graph dwarf,4096 \
  -o instructions.perf.data -- ./path/to/binary arg...
p2prof --event instructions -top ./path/to/binary instructions.perf.data
```

Frequency sampling is convenient for collecting unlike events together because
each event gets comparable sample density. For quantitative totals and ratios,
use `perf stat`; for controlled stack attribution, use separate recordings with
event-appropriate `-c` periods. The user guide explains these choices in detail.

For longer-running programs, prefer attaching to the process or selected
threads for a bounded interval:

```sh
perf record -F 19 -g --call-graph dwarf -p "$pid" -o profile.perf.data -- sleep 60
perf record -F 19 -g --call-graph dwarf -t "$tid_list" -o profile.perf.data -- sleep 60
```

DWARF stack capture is more expensive than frame-pointer unwinding because perf
copies user register and stack bytes for each sample. To reduce overhead:

- lower the sample rate with `-F 19` or `-F 49`
- use an event period instead of a frequency, for example `-c 10000000`
- bound copied stack bytes, for example `--call-graph dwarf,4096`
- attach only to the relevant process or TIDs with `-p` or `-t`
- record only during the workload window you care about
- keep debug information available for `perf script` symbolization

Example lower-overhead collection:

```sh
perf record -c 10000000 -g --call-graph dwarf,4096 \
  -t "$tid_list" \
  -o profile.perf.data \
  -- sleep 60
```

## p2prof Command Model

`p2prof` preserves the pprof command-line shape while accepting Linux perf data
files directly. It recognizes perf data by its file header, so the input does
not need to be named exactly `perf.data`:

```sh
p2prof -top ./path/to/binary profile.perf.data
p2prof -png -output profile.png ./path/to/binary perf.data
```

For a perf data file containing more than one event, select the event before
conversion with `--event`. The wrapper passes the selector to `perf2pprof`,
which filters the parsed `perf script` samples before aggregating them, and
forwards all remaining arguments to pprof unchanged:

```sh
p2prof --event cycles -top ./path/to/binary multi-event.perf.data
p2prof --event=instructions -svg -output instructions.svg \
  ./path/to/binary multi-event.perf.data
```

The converter also accepts the selector directly:

```sh
perf2pprof --perf-data multi-event.perf.data --event cache-misses \
  -o cache-misses.pb.gz
```

Existing pprof profile inputs continue to pass through without conversion. Set
`PERF2PPROF` or `PPROF` to override either executable; otherwise `p2prof` first
looks for sibling binaries. For pprof it also checks `GOBIN` and `~/go/bin`
before falling back to `PATH`.

Add `--diagnose` to print a quick unwind/symbolization health check:

```sh
perf2pprof --perf-data profile.perf.data --diagnose -o profile.pb.gz
```

The diagnostics report checks for common bad signs:

- mostly shallow stacks
- many `[unknown]` frames
- many raw-address frames
- no `main` frames

It also prints a few sample multi-frame stacks so you can see whether `perf`
successfully unwound through application code.

The output contains two sample values:

- `samples/count`
- the perf event period, for example `cycles/cycles`

The event period is the default pprof sample type.

## Advanced Manual Conversion

Use `perf2pprof` directly only when a persistent `.pb.gz` profile is useful.
Convert an existing script dump:

```sh
perf script -i profile.perf.data > profile.perf-script.txt
perf2pprof --script profile.perf-script.txt -o profile.pb.gz
pprof -svg ./path/to/binary profile.pb.gz > profile.svg
```

Or let the converter invoke `perf script`:

```sh
perf2pprof --perf-data profile.perf.data -o profile.pb.gz
pprof -top ./path/to/binary profile.pb.gz
```

## Why perf2pprof Exists

The main use case is Linux `perf record --call-graph dwarf` data for optimized
production binaries built without frame pointers. Frame-pointer unwinding is
fast, but on x86-64 it reserves `%rbp`; DWARF call graphs preserve that register
for normal code generation while still collecting stack traces.

Google's `perf_to_profile` does not currently decode the DWARF-only
`PERF_SAMPLE_REGS_USER` / `PERF_SAMPLE_STACK_USER` payloads in `perf.data`.
`perf2pprof` instead consumes `perf script`, the kernel-tools-supported text
representation of the already-unwound stacks.

## Integration Test

Run the end-to-end zstd profiling smoke test with:

```sh
scripts/test-zstd-profile-benchmark.sh
```

It validates frequency and multi-counter reports plus their PNG graphs. The
assertions work with either named functions or unresolved raw addresses, so a
matching system debug-symbol package improves the output but is not required
for the test to pass.

Run the collection styles from the user guide as separate tests with:

```sh
scripts/test-user-guide-examples.sh all
```

Pass `frequency-fp`, `frequency-dwarf`, `counter-period`, or `multi-counter`
instead of `all` to run one case. These tests produce text and PNG output only;
they do not start a web interface.

GitHub Actions runs the Rust and shell tests, the benchmark smoke test, and
each user-guide case as a separate CI matrix job on Ubuntu. GitHub-hosted
runners substitute `cpu-clock`, `task-clock`, and `page-faults` for the hardware
events that their virtual PMU does not expose. Local runs keep the hardware-
counter defaults shown above.
