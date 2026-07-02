# perf2pprof

`perf2pprof` converts `perf script` stack output into a gzipped pprof
`profile.proto`.

The main use case is Linux `perf record --call-graph dwarf` data for optimized
production-style binaries built without frame pointers. Frame-pointer unwinding
is fast, but on x86-64 it reserves `%rbp` as the frame pointer and reduces the
general-purpose register budget available to the optimizer from 15 registers to
14. DWARF call graphs let you profile binaries compiled with
`-fomit-frame-pointer`, preserving that register for normal code generation
while still collecting stack traces.

Google's `perf_to_profile` does not currently decode the DWARF-only
`PERF_SAMPLE_REGS_USER` / `PERF_SAMPLE_STACK_USER` payloads in `perf.data`.
This tool avoids stale binary perf parsing by consuming `perf script`, which is
the kernel-tools-supported text representation of the already-unwound stacks.

## Installation

Install directly from GitHub with Cargo:

```sh
cargo install --git https://github.com/vladpetric/perf2pprof
```

Cargo installs the binary into `~/.cargo/bin` by default. Make sure that
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
```

## Collecting DWARF Perf Data

Use DWARF call graphs when the binary was built without frame pointers or when
you want to compare profile data without changing compiler flags:

```sh
perf record -F 49 -g --call-graph dwarf -o profile.perf.data -- ./path/to/binary arg...
```

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

## Usage

Convert an existing script dump:

```sh
perf script -i profile.perf.data > profile.perf-script.txt
perf2pprof --script profile.perf-script.txt -o profile.pb.gz
pprof -svg ./path/to/binary profile.pb.gz > profile.svg
```

Or let the tool invoke `perf script`:

```sh
perf2pprof --perf-data profile.perf.data -o profile.pb.gz
pprof -top ./path/to/binary profile.pb.gz
```

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
