# perf2pprof and p2prof User Guide

`perf2pprof` converts Linux `perf.data` call-chain samples into pprof profiles.
`p2prof` is the normal entry point: it accepts perf data directly, converts it
temporarily, and forwards the rest of the command line to Google's pprof.

## Requirements

- Linux `perf` from the kernel-tools package matching the running kernel
- a current upstream Google pprof build
- Graphviz `dot` for PNG, SVG, PDF, and web graph views
- Rust and Cargo for installing `perf2pprof` and `p2prof`
- debug information or resolvable symbols for useful function names

Do not use an old distro `pprof`, `google-pprof`, or the similarly named
gperftools script. Old versions lack web UI behavior and command-line features
used by this guide. Install the current `github.com/google/pprof` command with
Go instead.

## Install Modern pprof

Install a current Go toolchain, then build the latest upstream pprof:

```sh
go install github.com/google/pprof@latest
```

Go normally writes the binary to `$(go env GOPATH)/bin`. Add that directory to
`PATH`, or install a symlink in a directory already on `PATH`:

```sh
export PATH="$(go env GOPATH)/bin:$PATH"
mkdir -p ~/.local/bin
ln -sfn "$(go env GOPATH)/bin/pprof" ~/.local/bin/pprof
```

Google pprof does not consistently provide a `pprof --version` flag. Verify
the embedded Go module information instead:

```sh
go version -m "$(command -v pprof)"
```

The output must contain `path github.com/google/pprof` and a recent module
revision. Also verify Graphviz:

```sh
dot -V
pprof -help
```

The pprof help should include `-http`, `-no_browser`, `-png`, `-svg`, and
`-nodecount`. If it does not, replace that pprof build before using `p2prof`.

## Install perf2pprof and p2prof

Install directly from GitHub with Cargo:

```sh
cargo install --git https://github.com/vladpetric/perf2pprof --locked --force
```

Cargo installs both binaries from the package. Confirm both are present:

```sh
command -v perf2pprof
command -v p2prof
perf2pprof --version
p2prof -help
```

For a source checkout, use:

```sh
git clone https://github.com/vladpetric/perf2pprof
cd perf2pprof
cargo install --path . --locked --force
```

The release can also be built and installed explicitly:

```sh
cargo build --release
install -m 0755 target/release/perf2pprof ~/.local/bin/perf2pprof
install -m 0755 target/release/p2prof ~/.local/bin/p2prof
```

## Collect Perf Data

The normal workflow has two commands: collect with `perf record`, then inspect
the resulting file with `p2prof`. No intermediate conversion command or profile
file is required.

### Fixed-frequency sampling without DWARF

This requests 99 sampling interrupts per second. Frame-pointer unwinding has
low recording overhead, but complete stacks require the program and its
libraries to have been built with frame pointers:

```sh
perf record -F 99 -e cycles -g --call-graph fp \
  -o workload.perf.data -- ./workload
p2prof --event cycles -top -nodecount=20 \
  ./workload workload.perf.data
```

Generate a graph from the same recording by changing only the `p2prof` command:

```sh
p2prof --event cycles -png -nodecount=80 \
  -output workload.cycles.png ./workload workload.perf.data
```

### Fixed-frequency sampling with DWARF

DWARF call chains work with optimized programs that omit frame pointers. The
frequency remains 99 interrupts per second, while `dwarf,4096` bounds the user
stack copied for each sample:

```sh
perf record -F 99 -g --call-graph dwarf,4096 \
  -e cycles -o workload.perf.data -- ./workload
p2prof --event cycles -top -nodecount=20 \
  ./workload workload.perf.data
```

Use frame-pointer collection when it produces complete stacks; use DWARF when
frame pointers are absent or stacks are visibly truncated. Debug symbols make
function names more useful, but they are not required to record or convert a
profile.

### Low-level hardware counters

Collect several PMU events in one run when comparisons must cover the exact
same execution. Here each event is sampled at 99 interrupts per second:

```sh
perf record -F 99 -g --call-graph dwarf,4096 \
  -e instructions \
  -e branch-misses \
  -e L1-dcache-load-misses \
  -o workload.multi.perf.data -- ./workload
p2prof --event instructions -top -nodecount=20 \
  ./workload workload.multi.perf.data
p2prof --event branch-misses -top -nodecount=20 \
  ./workload workload.multi.perf.data
p2prof --event L1-dcache-load-misses -png -nodecount=80 \
  -output workload.l1-misses.png ./workload workload.multi.perf.data
```

For one hardware event, a fixed counter period can replace the requested
interrupt frequency. This samples once per 10 million retired instructions:

```sh
perf record -c 10000000 -e instructions -g --call-graph dwarf,4096 \
  -o workload.instructions.perf.data -- ./workload
p2prof --event instructions -top -nodecount=20 \
  ./workload workload.instructions.perf.data
```

Hardware event names and availability are CPU-specific. If a named event is
unsupported, select a corresponding event listed by the local `perf` tool.

## p2prof Command Model

`p2prof` recognizes perf data from the `PERFILE2` file header, not its
filename. `perf.data`, `workload.perf.data`, and renamed captures are accepted.
Existing `.pb.gz` pprof profiles are passed through unchanged.

`p2prof` consumes one wrapper-specific option:

```text
--event EVENT
--event=EVENT
```

Use it to select one stream from a multi-event perf capture. A selector such
as `cycles` also matches a recorded modifier such as `cycles:P`. Without an
event selector, a capture containing incompatible event streams is rejected
instead of combining their periods.

Every other argument is forwarded to pprof in the original order. pprof uses
mostly single-hyphen options such as `-top`, `-png`, and `-nodecount=10`.

`p2prof` locates sibling `perf2pprof` and pprof executables first. It also
checks `GOBIN` and `~/go/bin` for pprof, then falls back to `PATH`. Override
either command explicitly when needed:

```sh
PERF2PPROF=~/bin/perf2pprof PPROF=~/go/bin/pprof \
  p2prof --event cycles -top ./workload workload.perf.data
```

## Text Reports

Show the ten functions with the most sampled cycles:

```sh
p2prof --event cycles -top -nodecount=10 \
  ./workload workload.multi.perf.data
```

Show the largest instruction counts:

```sh
p2prof --event instructions -top -nodecount=10 \
  ./workload workload.multi.perf.data
```

Attribute L1 data-cache load misses:

```sh
p2prof --event L1-dcache-load-misses -top -nodecount=10 \
  ./workload workload.multi.perf.data
```

Sort by cumulative cost rather than flat cost:

```sh
p2prof --event cycles -top -cum -nodecount=20 \
  ./workload workload.multi.perf.data
```

Restrict a report to stacks involving a function and hide runtime noise:

```sh
p2prof --event cycles -top -nodecount=20 \
  -focus='compressStream' -ignore='pthread|clone' \
  ./workload workload.multi.perf.data
```

The options `-cum`, `-nodecount`, `-focus`, and `-ignore` above are ordinary
pprof options passed through by `p2prof`.

## Static Graphs

Create a directed call graph as PNG:

```sh
p2prof --event branch-misses -png -nodecount=80 \
  -output /tmp/branch-misses.png \
  ./workload workload.multi.perf.data
```

Display it in Kitty:

```sh
kitten icat /tmp/branch-misses.png
```

Create scalable SVG output:

```sh
p2prof --event L1-dcache-load-misses -svg -nodecount=100 \
  -output /tmp/l1-misses.svg \
  ./workload workload.multi.perf.data
```

Reduce graph noise with pprof's trimming controls:

```sh
p2prof --event cycles -png -nodecount=60 \
  -nodefraction=0.005 -edgefraction=0.001 \
  -output /tmp/cycles.png ./workload workload.multi.perf.data
```

## Interactive Web UI

Run a loopback-only server without trying to launch a browser on the profiling
host:

```sh
p2prof --event branch-misses -http=127.0.0.1:8080 -no_browser \
  ./workload workload.multi.perf.data
```

Forward it from another machine:

```sh
ssh -N -L 8080:127.0.0.1:8080 user@profiling-host
```

Open `http://localhost:8080/ui/graph` for the directed graph. The `/ui/` root
redirects to the flame graph in current pprof builds.

For direct Tailscale access, bind the profiling host's Tailscale address rather
than all interfaces:

```sh
p2prof --event branch-misses -http=100.x.y.z:8080 -no_browser \
  ./workload workload.multi.perf.data
```

## Direct perf2pprof Use

Use `perf2pprof` directly when a persistent `.pb.gz` profile is useful:

```sh
perf2pprof --perf-data workload.multi.perf.data \
  --event cycles -o cycles.pb.gz
pprof -top -nodecount=10 ./workload cycles.pb.gz
```

Additional `perf script` arguments can be passed with repeated `--perf-arg`
options. Event selection is performed by `perf2pprof` after parsing because
many installed `perf script` versions do not provide an event-filter option.

## Reproducible zstd Profiling Benchmark

The repository includes a deterministic text generator and an end-to-end zstd
benchmark. The default run generates slightly more than 400 MiB of newline-
terminated text from a fixed 200-word `a`/`b` lexicon. Each line contains 200
to 1000 characters.

Run the full benchmark:

```sh
scripts/zstd-profile-benchmark.sh
```

Run a smaller smoke test in a named directory:

```sh
scripts/zstd-profile-benchmark.sh \
  --size-mib 32 --frequency 49 --out /tmp/p2prof-zstd-smoke
```

Run the repository integration test:

```sh
scripts/test-zstd-profile-benchmark.sh
```

The test validates every top report and PNG graph. It detects and reports
whether zstd names were resolved, but deliberately accepts both symbolized
function names and raw `[unknown]` addresses. Set `KEEP_ARTIFACTS=1` to retain
its temporary output or reduce runtime with `SIZE_MIB=16`.

### User-guide example tests

The example test suite uses the same deterministic zstd compression and
byte-verified decompression workload for each collection style shown above.
Run every case or select one:

```sh
scripts/test-user-guide-examples.sh all
scripts/test-user-guide-examples.sh frequency-fp
scripts/test-user-guide-examples.sh frequency-dwarf
scripts/test-user-guide-examples.sh counter-period
scripts/test-user-guide-examples.sh multi-counter
```

Each case invokes `p2prof` to create top reports and static PNG graphs for both
compression and decompression. The tests never start the pprof web interface.
They accept symbolized or unresolved frames. Set `KEEP_ARTIFACTS=1` to retain
the generated perf data, reports, and graphs.

GitHub Actions runs every case. Because GitHub-hosted virtual machines do not
expose all hardware PMU events, CI substitutes `cpu-clock`, `task-clock`, and
`page-faults` to exercise fixed-period and simultaneous multi-event recording.
The script defaults remain `cycles`, `instructions`, `branch-misses`, and
`L1-dcache-load-misses` for hardware-backed local runs.

The runner uses system `zstd` quietly in single-threaded mode. It records and
verifies both compression and decompression. For each workload it collects:

- a frequency-sampled cycles profile;
- one simultaneous recording of instructions, branch misses, and L1 data-
  cache load misses;
- a top-10 text report for every event; and
- a directed PNG call graph for every event.

The output directory retains the source text, compressed archives, perf data,
checksums, parameters, top reports, and graphs. Every decompression stream is
compared byte-for-byte with the generated source before the run succeeds.

Many distributions strip local function symbols from `/usr/bin/zstd`. Install
the matching zstd debug-symbol package before recording when reports must show
internal zstd function names. On Ubuntu this is normally `zstd-dbgsym` from the
Ubuntu debug-symbol repository. Without matching symbols, event totals remain
valid but internal nodes are displayed as raw `[unknown]` addresses.

## Troubleshooting

### Mixed events

If conversion reports multiple perf events, add the recorded event name:

```sh
p2prof --event cycles -top ./workload workload.multi.perf.data
```

### No graph output

Confirm `dot -V` succeeds. PNG, SVG, PDF, and the web graph require Graphviz.

### Unknown frames

Keep the exact profiled binary and its debug information. Kernel frames may
remain unresolved when `/proc/kallsyms` is restricted; that is independent of
the pprof conversion.

### Web UI is unreachable remotely

`-http=:8080` defaults to localhost in pprof. Use an SSH tunnel or explicitly
bind the Tailscale address. Add `-no_browser` on headless hosts.
