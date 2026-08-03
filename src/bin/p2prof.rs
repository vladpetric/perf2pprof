use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result, anyhow, bail};

const PERF_MAGIC: &[u8; 8] = b"PERFILE2";
const PERF_MAGIC_REVERSED: &[u8; 8] = b"2ELIFREP";

fn main() {
    match run(env::args_os().skip(1)) {
        Ok(status) => exit_with_status(status),
        Err(err) => {
            eprintln!("p2prof: {err:#}");
            std::process::exit(1);
        }
    }
}

fn run(args: impl IntoIterator<Item = OsString>) -> Result<ExitStatus> {
    let (event, args) = parse_args(args)?;
    let perf_inputs = args
        .iter()
        .filter(|arg| is_perf_data(Path::new(arg.as_os_str())))
        .count();

    if perf_inputs == 0 {
        if event.is_some() {
            bail!("--event requires at least one perf.data input");
        }
        return run_pprof(args);
    }

    let tempdir = tempfile::tempdir().context("create temporary profile directory")?;
    let perf2pprof = tool_path("PERF2PPROF", "perf2pprof");
    let event = event
        .as_deref()
        .map(|value| {
            value
                .to_str()
                .ok_or_else(|| anyhow!("--event must be valid UTF-8"))
        })
        .transpose()?;
    let mut converted_args = Vec::with_capacity(args.len());
    let mut profile_index = 0;

    for arg in args {
        let input = Path::new(&arg);
        if !is_perf_data(input) {
            converted_args.push(arg);
            continue;
        }

        let output = tempdir
            .path()
            .join(format!("profile-{profile_index}.pb.gz"));
        profile_index += 1;
        let mut converter = Command::new(&perf2pprof);
        converter
            .arg("--perf-data")
            .arg(input)
            .arg("-o")
            .arg(&output);
        if let Some(event) = event {
            converter.arg("--event").arg(event);
        }
        let status = converter
            .status()
            .with_context(|| format!("run {}", Path::new(&perf2pprof).display()))?;
        if !status.success() {
            bail!("perf2pprof failed for {} with {status}", input.display());
        }
        converted_args.push(output.into_os_string());
    }

    run_pprof(converted_args)
}

fn parse_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<(Option<OsString>, Vec<OsString>)> {
    let mut args = args.into_iter();
    let mut event = None;
    let mut forwarded = Vec::new();

    while let Some(arg) = args.next() {
        if arg == OsStr::new("--event") {
            if event.is_some() {
                bail!("--event may only be specified once");
            }
            event = Some(
                args.next()
                    .ok_or_else(|| anyhow!("--event requires a value"))?,
            );
        } else if let Some(value) = arg.to_str().and_then(|arg| arg.strip_prefix("--event=")) {
            if event.is_some() {
                bail!("--event may only be specified once");
            }
            if value.is_empty() {
                bail!("--event requires a value");
            }
            event = Some(value.into());
        } else {
            forwarded.push(arg);
        }
    }

    Ok((event, forwarded))
}

fn is_perf_data(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut magic = [0_u8; 8];
    file.read_exact(&mut magic).is_ok() && (&magic == PERF_MAGIC || &magic == PERF_MAGIC_REVERSED)
}

fn run_pprof(args: Vec<OsString>) -> Result<ExitStatus> {
    let pprof = tool_path("PPROF", "pprof");
    Command::new(&pprof)
        .args(args)
        .status()
        .with_context(|| format!("run {}", Path::new(&pprof).display()))
}

fn tool_path(environment: &str, binary: &str) -> OsString {
    if let Some(path) = env::var_os(environment) {
        return path;
    }
    if let Ok(current) = env::current_exe()
        && let Some(parent) = current.parent()
    {
        let candidate = parent.join(binary);
        if candidate.is_file() {
            return candidate.into_os_string();
        }
    }
    if binary == "pprof" {
        let mut roots = Vec::new();
        if let Some(gobin) = env::var_os("GOBIN") {
            roots.push(PathBuf::from(gobin));
        }
        if let Some(home) = env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("go/bin"));
        }
        for root in roots {
            let candidate = root.join(binary);
            if candidate.is_file() {
                return candidate.into_os_string();
            }
        }
    }
    binary.into()
}

fn exit_with_status(status: ExitStatus) -> ! {
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn extracts_event_and_preserves_pprof_arguments() {
        let args = ["-png", "--event=instructions", "binary", "perf.data"]
            .into_iter()
            .map(OsString::from);
        let (event, forwarded) = parse_args(args).expect("parse arguments");

        assert_eq!(event, Some(OsString::from("instructions")));
        assert_eq!(
            forwarded,
            ["-png", "binary", "perf.data"].map(OsString::from)
        );
    }

    #[test]
    fn recognizes_perf_data_by_magic_not_filename() {
        let mut file = tempfile::NamedTempFile::new().expect("temporary file");
        file.write_all(PERF_MAGIC).expect("write perf magic");
        file.write_all(b"payload").expect("write payload");

        assert!(is_perf_data(file.path()));
    }

    #[test]
    fn does_not_treat_an_ordinary_data_file_as_perf_data() {
        let mut file = tempfile::Builder::new()
            .suffix(".data")
            .tempfile()
            .expect("temporary file");
        file.write_all(b"not perf data").expect("write data");

        assert!(!is_perf_data(file.path()));
    }
}
