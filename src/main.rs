use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use flate2::Compression;
use flate2::write::GzEncoder;
use prost::Message;

pub mod perftools {
    pub mod profiles {
        include!(concat!(env!("OUT_DIR"), "/perftools.profiles.rs"));
    }
}

use perftools::profiles::{Function, Label, Line, Location, Mapping, Profile, Sample, ValueType};

#[derive(Debug, Parser)]
#[command(version, about = "Convert perf script stacks into pprof profile.proto")]
struct Args {
    /// Read an existing perf script text file. Use '-' for stdin.
    #[arg(long, conflicts_with = "perf_data")]
    script: Option<PathBuf>,

    /// Run perf script on this perf.data file and convert the output.
    #[arg(long, conflicts_with = "script")]
    perf_data: Option<PathBuf>,

    /// Output gzipped pprof profile. Defaults to stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Extra argument passed to perf script. Repeatable.
    #[arg(long = "perf-arg", requires = "perf_data", allow_hyphen_values = true)]
    perf_args: Vec<String>,

    /// Add a free-form pprof comment. Repeatable.
    #[arg(long = "comment")]
    comments: Vec<String>,

    /// Print stack unwind/symbolization diagnostics to stderr.
    #[arg(long)]
    diagnose: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Frame {
    function: String,
    address: u64,
    mapping: String,
}

#[derive(Debug, Clone)]
struct RawSample {
    comm: String,
    pid: i64,
    tid: i64,
    event: String,
    period: i64,
    stack: Vec<Frame>,
}

#[derive(Debug, Clone, Default)]
struct AggSample {
    samples: i64,
    period: i64,
    comm: String,
    pid: i64,
    tid: i64,
    event: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StackKey(Vec<Frame>);

struct StringTable {
    values: Vec<String>,
    ids: HashMap<String, i64>,
}

impl StringTable {
    fn new() -> Self {
        let mut table = Self {
            values: Vec::new(),
            ids: HashMap::new(),
        };
        table.intern("");
        table
    }

    fn intern(&mut self, value: impl AsRef<str>) -> i64 {
        let value = value.as_ref();
        if let Some(id) = self.ids.get(value) {
            return *id;
        }
        let id = self.values.len() as i64;
        self.values.push(value.to_string());
        self.ids.insert(value.to_string(), id);
        id
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let samples = read_samples(&args)?;
    if samples.is_empty() {
        return Err(anyhow!("no perf samples found"));
    }

    if args.diagnose {
        diagnose_samples(&samples);
    }

    let profile = build_profile(samples, &args.comments)?;
    write_profile(&profile, args.output.as_ref())?;
    Ok(())
}

fn read_samples(args: &Args) -> Result<Vec<RawSample>> {
    if let Some(path) = &args.script {
        if path.as_os_str() == "-" {
            let stdin = std::io::stdin();
            return parse_perf_script(stdin.lock());
        }

        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        return parse_perf_script(BufReader::new(file));
    }

    if let Some(path) = &args.perf_data {
        let mut cmd = Command::new("perf");
        cmd.arg("script").arg("-i").arg(path);
        cmd.args(&args.perf_args);
        cmd.stdout(Stdio::piped());
        let mut child = cmd.spawn().with_context(|| "spawn perf script")?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("perf script stdout unavailable"))?;
        let samples = parse_perf_script(BufReader::new(stdout))?;
        let status = child.wait().with_context(|| "wait for perf script")?;
        if !status.success() {
            return Err(anyhow!("perf script exited with {}", status));
        }
        return Ok(samples);
    }

    let stdin = std::io::stdin();
    parse_perf_script(stdin.lock())
}

fn parse_perf_script<R: BufRead>(reader: R) -> Result<Vec<RawSample>> {
    let mut samples = Vec::new();
    let mut current: Option<RawSample> = None;

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            flush_sample(&mut samples, current.take());
            continue;
        }

        if let Some(next_sample) = parse_header(&line) {
            flush_sample(&mut samples, current.take());
            current = Some(next_sample);
            continue;
        }

        if line.starts_with('\t') || line.starts_with("        ") {
            if let Some(sample) = current.as_mut() {
                if let Some(frame) = parse_frame(&line) {
                    sample.stack.push(frame);
                }
            }
            continue;
        }
    }

    flush_sample(&mut samples, current);

    Ok(samples)
}

fn flush_sample(samples: &mut Vec<RawSample>, sample: Option<RawSample>) {
    if let Some(sample) = sample {
        if !sample.stack.is_empty() {
            samples.push(sample);
        }
    }
}

fn parse_header(line: &str) -> Option<RawSample> {
    let (left, right) = line.split_once(':')?;
    let right = right.trim();
    let mut right_tokens = right.split_whitespace();
    let period = right_tokens.next()?.parse::<i64>().ok()?;
    let event = right_tokens.next().unwrap_or("event").trim_end_matches(':');

    let mut left_tokens: Vec<&str> = left.split_whitespace().collect();
    if left_tokens.len() < 3 {
        return None;
    }

    let _time = left_tokens.pop()?;
    let pid_tid = left_tokens.pop()?;
    let comm = left_tokens.join(" ");
    let (pid, tid) = parse_pid_tid(pid_tid);

    let mut stack = Vec::new();
    let inline_frame = right_tokens.collect::<Vec<_>>().join(" ");
    if !inline_frame.is_empty() {
        if let Some(frame) = parse_frame(&inline_frame) {
            stack.push(frame);
        }
    }

    Some(RawSample {
        comm,
        pid,
        tid,
        event: event.to_string(),
        period,
        stack,
    })
}

fn parse_pid_tid(value: &str) -> (i64, i64) {
    if let Some((pid, tid)) = value.split_once('/') {
        return (
            pid.parse::<i64>().unwrap_or(0),
            tid.parse::<i64>().unwrap_or(0),
        );
    }
    let pid = value.parse::<i64>().unwrap_or(0);
    (pid, pid)
}

fn parse_frame(line: &str) -> Option<Frame> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let addr_text = parts.next()?;
    let rest = parts.next().unwrap_or("[unknown]").trim();
    let address = u64::from_str_radix(addr_text.trim_start_matches("0x"), 16).unwrap_or(0);

    let (function, mapping) = if let Some(open) = rest.rfind(" (") {
        let function = rest[..open].trim();
        let mapping = rest[open + 2..].trim_end_matches(')').trim();
        (function, mapping)
    } else {
        (rest, "")
    };

    let function = strip_symbol_offset(function);
    let function = if function.is_empty() {
        format!("0x{address:x}")
    } else if function == "[unknown]" && address != 0 {
        format!("[unknown] 0x{address:x}")
    } else {
        function.to_string()
    };

    Some(Frame {
        function,
        address,
        mapping: mapping.to_string(),
    })
}

fn strip_symbol_offset(function: &str) -> &str {
    let Some((base, offset)) = function.rsplit_once("+0x") else {
        return function;
    };
    if !offset.is_empty() && offset.bytes().all(|b| b.is_ascii_hexdigit()) {
        base
    } else {
        function
    }
}

fn build_profile(samples: Vec<RawSample>, comments: &[String]) -> Result<Profile> {
    let mut aggregate: HashMap<StackKey, AggSample> = HashMap::new();
    let mut event_name = "event".to_string();

    for sample in samples {
        if sample.stack.is_empty() {
            continue;
        }
        event_name = sample.event.clone();
        let key = StackKey(sample.stack);
        let entry = aggregate.entry(key).or_insert_with(|| AggSample {
            comm: sample.comm.clone(),
            pid: sample.pid,
            tid: sample.tid,
            event: sample.event.clone(),
            ..AggSample::default()
        });
        entry.samples += 1;
        entry.period += sample.period;
    }

    let mut strings = StringTable::new();
    let samples_id = strings.intern("samples");
    let count_id = strings.intern("count");
    let event_id = strings.intern(normalize_event_name(&event_name));
    let event_unit_id = strings.intern(event_unit_name(&event_name));
    let synthetic_mapping_file = strings.intern("[perf script]");

    let mut profile = Profile {
        sample_type: vec![
            ValueType {
                r#type: samples_id,
                unit: count_id,
            },
            ValueType {
                r#type: event_id,
                unit: event_unit_id,
            },
        ],
        mapping: vec![Mapping {
            id: 1,
            memory_start: 0,
            memory_limit: u64::MAX,
            file_offset: 0,
            filename: synthetic_mapping_file,
            build_id: 0,
            has_functions: true,
            has_filenames: false,
            has_line_numbers: false,
            has_inline_frames: false,
        }],
        period_type: Some(ValueType {
            r#type: event_id,
            unit: event_unit_id,
        }),
        default_sample_type: event_id,
        ..Profile::default()
    };

    let mut function_ids: HashMap<String, u64> = HashMap::new();
    let mut location_ids: HashMap<Frame, u64> = HashMap::new();
    let mut next_function_id = 1_u64;
    let mut next_location_id = 1_u64;

    let label_comm = strings.intern("comm");
    let label_pid = strings.intern("pid");
    let label_tid = strings.intern("tid");
    let label_event = strings.intern("event");

    for (key, agg) in aggregate {
        let mut location_id = Vec::with_capacity(key.0.len());
        for frame in key.0 {
            let loc_id = if let Some(id) = location_ids.get(&frame) {
                *id
            } else {
                let function_id = if let Some(id) = function_ids.get(&frame.function) {
                    *id
                } else {
                    let id = next_function_id;
                    next_function_id += 1;
                    let name = strings.intern(&frame.function);
                    profile.function.push(Function {
                        id,
                        name,
                        system_name: name,
                        filename: strings.intern(&frame.mapping),
                        start_line: 0,
                    });
                    function_ids.insert(frame.function.clone(), id);
                    id
                };

                let id = next_location_id;
                next_location_id += 1;
                profile.location.push(Location {
                    id,
                    mapping_id: 1,
                    address: frame.address,
                    line: vec![Line {
                        function_id,
                        line: 0,
                        column: 0,
                    }],
                    is_folded: false,
                });
                location_ids.insert(frame, id);
                id
            };
            location_id.push(loc_id);
        }

        profile.sample.push(Sample {
            location_id,
            value: vec![agg.samples, agg.period],
            label: vec![
                Label {
                    key: label_comm,
                    str: strings.intern(&agg.comm),
                    num: 0,
                    num_unit: 0,
                },
                Label {
                    key: label_pid,
                    str: 0,
                    num: agg.pid,
                    num_unit: count_id,
                },
                Label {
                    key: label_tid,
                    str: 0,
                    num: agg.tid,
                    num_unit: count_id,
                },
                Label {
                    key: label_event,
                    str: strings.intern(&agg.event),
                    num: 0,
                    num_unit: 0,
                },
            ],
        });
    }

    for comment in comments {
        let id = strings.intern(comment);
        profile.comment.push(id);
    }
    profile
        .comment
        .push(strings.intern("generated by perf2pprof from perf script stacks"));
    profile.string_table = strings.values;
    Ok(profile)
}

fn normalize_event_name(event: &str) -> String {
    event.trim_end_matches(':').replace(':', "_")
}

fn event_unit_name(event: &str) -> &'static str {
    if event.contains("cycles") {
        "cycles"
    } else if event.contains("instructions") {
        "instructions"
    } else {
        "count"
    }
}

fn write_profile(profile: &Profile, output: Option<&PathBuf>) -> Result<()> {
    let mut encoded = Vec::new();
    profile.encode(&mut encoded)?;

    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    gzip.write_all(&encoded)?;
    let compressed = gzip.finish()?;

    match output {
        Some(path) => {
            let mut file =
                File::create(path).with_context(|| format!("create output {}", path.display()))?;
            file.write_all(&compressed)?;
        }
        None => {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(&compressed)?;
        }
    }
    Ok(())
}

fn diagnose_samples(samples: &[RawSample]) {
    let sample_count = samples.len();
    let frame_count: usize = samples.iter().map(|sample| sample.stack.len()).sum();
    let shallow_count = samples
        .iter()
        .filter(|sample| sample.stack.len() <= 2)
        .count();
    let unknown_frame_count = samples
        .iter()
        .flat_map(|sample| sample.stack.iter())
        .filter(|frame| is_unknown_frame(&frame.function))
        .count();
    let raw_address_frame_count = samples
        .iter()
        .flat_map(|sample| sample.stack.iter())
        .filter(|frame| is_raw_address_frame(&frame.function))
        .count();
    let app_frame_count = samples
        .iter()
        .flat_map(|sample| sample.stack.iter())
        .filter(|frame| looks_like_application_frame(&frame.function))
        .count();

    let average_depth = if sample_count == 0 {
        0.0
    } else {
        frame_count as f64 / sample_count as f64
    };
    let unknown_pct = percent(unknown_frame_count, frame_count);
    let raw_address_pct = percent(raw_address_frame_count, frame_count);
    let shallow_pct = percent(shallow_count, sample_count);

    eprintln!("perf2pprof diagnostics:");
    eprintln!("  samples: {sample_count}");
    eprintln!("  frames: {frame_count}");
    eprintln!("  average stack depth: {average_depth:.2}");
    eprintln!("  shallow samples (<=2 frames): {shallow_count} ({shallow_pct:.1}%)");
    eprintln!("  unknown frames: {unknown_frame_count} ({unknown_pct:.1}%)");
    eprintln!("  raw-address frames: {raw_address_frame_count} ({raw_address_pct:.1}%)");
    eprintln!("  main frames: {app_frame_count}");

    let mut bad_signs = Vec::new();
    if average_depth < 4.0 || shallow_pct > 40.0 {
        bad_signs.push("many samples are shallow; DWARF unwind may have failed or frame data may be incomplete");
    }
    if unknown_pct > 20.0 {
        bad_signs.push(
            "many frames are [unknown]; debug symbols, build IDs, or mappings may be missing",
        );
    }
    if raw_address_pct > 20.0 {
        bad_signs.push("many frames are raw addresses; symbolization may be incomplete");
    }
    if app_frame_count == 0 {
        bad_signs.push("no main frames found; check that perf can find the profiled binary and debug info");
    }

    if bad_signs.is_empty() {
        eprintln!("  status: no obvious bad signs");
    } else {
        eprintln!("  bad signs:");
        for sign in bad_signs {
            eprintln!("    - {sign}");
        }
    }

    eprintln!("  sample useful stacks:");
    let useful_samples = samples
        .iter()
        .filter(|sample| {
            sample.stack.len() >= 4
                && sample
                    .stack
                    .iter()
                    .any(|frame| looks_like_application_frame(&frame.function))
        })
        .take(3)
        .collect::<Vec<_>>();
    let useful_samples = if useful_samples.is_empty() {
        samples
            .iter()
            .filter(|sample| sample.stack.len() >= 4)
            .take(3)
            .collect::<Vec<_>>()
    } else {
        useful_samples
    };
    for sample in useful_samples {
        let rendered = sample
            .stack
            .iter()
            .map(|frame| frame.function.as_str())
            .collect::<Vec<_>>()
            .join(" <- ");
        eprintln!("    - {rendered}");
    }
}

fn percent(count: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        (count as f64 * 100.0) / total as f64
    }
}

fn is_unknown_frame(function: &str) -> bool {
    function.contains("[unknown]")
}

fn is_raw_address_frame(function: &str) -> bool {
    let value = function
        .strip_prefix("0x")
        .or_else(|| function.strip_prefix("[unknown] 0x"));
    value
        .map(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or(false)
}

fn looks_like_application_frame(function: &str) -> bool {
    function == "main"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_perf_script_sample() {
        let input = b"test_bin 123/456 10.25: 99 cycles:P:\n\t400123 leaf+0x10 (/bin/test)\n\t400100 caller (/bin/test)\n\n";
        let samples = parse_perf_script(&input[..]).expect("parse");
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].comm, "test_bin");
        assert_eq!(samples[0].pid, 123);
        assert_eq!(samples[0].tid, 456);
        assert_eq!(samples[0].period, 99);
        assert_eq!(samples[0].event, "cycles:P");
        assert_eq!(samples[0].stack[0].function, "leaf");
        assert_eq!(samples[0].stack[0].address, 0x400123);
        assert_eq!(samples[0].stack[0].mapping, "/bin/test");
    }

    #[test]
    fn parses_padded_one_line_perf_script_samples() {
        let input = b"       perf-exec  185184 1272593.206833:         16 cycles:P:  ffffffff9e4c7c44 [unknown] ([unknown])\n test_fullnode_a  185184 1272593.208016:    1513175 cycles:P:      75f939c28504 strlen@plt+0x4 (/usr/lib/x86_64-linux-gnu/libc.so.6)\n";
        let samples = parse_perf_script(&input[..]).expect("parse");
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].comm, "perf-exec");
        assert_eq!(samples[0].period, 16);
        assert_eq!(samples[0].stack.len(), 1);
        assert_eq!(samples[0].stack[0].function, "[unknown] 0xffffffff9e4c7c44");
        assert_eq!(samples[1].comm, "test_fullnode_a");
        assert_eq!(samples[1].period, 1513175);
        assert_eq!(samples[1].stack.len(), 1);
        assert_eq!(samples[1].stack[0].function, "strlen@plt");
        assert_eq!(samples[1].stack[0].mapping, "/usr/lib/x86_64-linux-gnu/libc.so.6");
    }

    #[test]
    fn builds_pprof_with_leaf_first_locations() {
        let samples = vec![RawSample {
            comm: "test_bin".to_string(),
            pid: 1,
            tid: 1,
            event: "cycles:P".to_string(),
            period: 7,
            stack: vec![
                Frame {
                    function: "leaf".to_string(),
                    address: 0x20,
                    mapping: "/bin/test".to_string(),
                },
                Frame {
                    function: "caller".to_string(),
                    address: 0x10,
                    mapping: "/bin/test".to_string(),
                },
            ],
        }];

        let profile = build_profile(samples, &[]).expect("profile");
        assert_eq!(profile.sample.len(), 1);
        assert_eq!(profile.sample[0].value, vec![1, 7]);
        assert_eq!(profile.sample[0].location_id.len(), 2);

        let leaf_loc = profile.sample[0].location_id[0];
        let location = profile
            .location
            .iter()
            .find(|loc| loc.id == leaf_loc)
            .expect("leaf location");
        let function = profile
            .function
            .iter()
            .find(|function| function.id == location.line[0].function_id)
            .expect("leaf function");
        assert_eq!(profile.string_table[function.name as usize], "leaf");
    }

    #[test]
    fn detects_raw_address_frames() {
        assert!(is_raw_address_frame("0x123abc"));
        assert!(is_raw_address_frame("[unknown] 0x123abc"));
        assert!(!is_raw_address_frame("example_function"));
    }
}
