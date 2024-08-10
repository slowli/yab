//! `cachegrind`-related logic.

use std::{
    borrow::Cow,
    collections::HashMap,
    fs, io,
    io::BufRead,
    path::Path,
    process::{Command, Stdio},
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum CachegrindError {
    #[error("I/O error executing cachegrind: {0}")]
    Exec(#[source] io::Error),
    #[error(
        "Unable to get `cachegrind` version. Please make sure that `valgrind` is installed \
         and is on PATH"
    )]
    NoCachegrind,

    #[error("I/O error creating output directory `{path}`: {error}")]
    CreateOutputDir {
        path: String,
        #[source]
        error: io::Error,
    },
    #[error("I/O error reading cachegrind output at `{out_path}`: {error}")]
    Read {
        out_path: String,
        #[source]
        error: io::Error,
    },
    #[error("Failed parsing cachegrind output at `{out_path}`: {message}")]
    Parse {
        out_path: String,
        message: Cow<'static, str>,
    },
}

enum ParseError {
    Custom(Cow<'static, str>),
    Io(io::Error),
}

impl ParseError {
    fn generalize(self, out_path: String) -> CachegrindError {
        match self {
            Self::Io(error) => CachegrindError::Read { out_path, error },
            Self::Custom(message) => CachegrindError::Parse { out_path, message },
        }
    }
}

impl From<io::Error> for ParseError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<&'static str> for ParseError {
    fn from(message: &'static str) -> Self {
        Self::Custom(message.into())
    }
}

impl From<String> for ParseError {
    fn from(message: String) -> Self {
        Self::Custom(message.into())
    }
}

pub(crate) fn check() -> Result<(), CachegrindError> {
    let output = Command::new("valgrind")
        .args(["--tool=cachegrind", "--version"])
        .output()
        .map_err(CachegrindError::Exec)?;
    if !output.status.success() {
        return Err(CachegrindError::NoCachegrind);
    }
    // FIXME: check version
    Ok(())
}

pub(crate) fn spawn_instrumented(
    mut command: Command,
    out_path: &str,
    this_executable: &str,
    name: &str,
) -> Result<CachegrindSummary, CachegrindError> {
    if let Some(parent_dir) = Path::new(out_path).parent() {
        fs::create_dir_all(parent_dir).map_err(|error| CachegrindError::CreateOutputDir {
            path: parent_dir.display().to_string(),
            error,
        })?;
    }

    command.args([this_executable, "--cachegrind-instrument", "--exact", name]);
    let status = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(CachegrindError::Exec)?;
    if !status.success() {
        let err = io::Error::new(
            io::ErrorKind::Other,
            format!("Failed running cachegrind, exit code: {status}"),
        );
        return Err(CachegrindError::Exec(err));
    }

    let out = fs::File::open(out_path).map_err(|error| CachegrindError::Read {
        out_path: out_path.to_owned(),
        error,
    })?;
    CachegrindSummary::read(io::BufReader::new(out))
        .map_err(|err| err.generalize(out_path.to_owned()))
}

#[derive(Debug, Clone, Copy)]
struct CachegrindDataPoint {
    total: u64,
    l1_misses: u64,
    l3_misses: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CachegrindSummary {
    instructions: CachegrindDataPoint,
    data_reads: CachegrindDataPoint,
    data_writes: CachegrindDataPoint,
}

impl CachegrindSummary {
    pub fn new(file: fs::File, path: &str) -> Result<Self, CachegrindError> {
        let reader = io::BufReader::new(file);
        Self::read(reader).map_err(|err| err.generalize(path.to_owned()))
    }

    fn read(reader: impl BufRead) -> Result<Self, ParseError> {
        let mut events_line = None;
        let mut summary_line = None;
        for line in reader.lines() {
            let line = line?;
            if let Some(events) = line.strip_prefix("events:") {
                if events_line.is_some() {
                    return Err("events are redefined".into());
                }
                events_line = Some(events.to_owned());
            } else if let Some(summary) = line.strip_prefix("summary:") {
                if summary_line.is_some() {
                    return Err("summary is redefined".into());
                }
                summary_line = Some(summary.to_owned());
            }
        }

        let events = events_line.ok_or("no events")?;
        let events: Vec<_> = events.split_whitespace().collect();
        let summary = summary_line.ok_or("no summary")?;
        let summary: Vec<_> = summary
            .split_whitespace()
            .map(|num| {
                num.parse::<u64>()
                    .map_err(|_| format!("summary is not an u64: {num}"))
            })
            .collect::<Result<_, _>>()?;
        if events.len() != summary.len() {
            return Err("mismatch between events and summary".into());
        }

        let summary_by_event: HashMap<_, _> = events.into_iter().zip(summary).collect();
        Ok(Self {
            instructions: CachegrindDataPoint {
                total: Self::summary_from_map(&summary_by_event, "Ir")?,
                l1_misses: Self::summary_from_map(&summary_by_event, "I1mr")?,
                l3_misses: Self::summary_from_map(&summary_by_event, "ILmr")?,
            },
            data_reads: CachegrindDataPoint {
                total: Self::summary_from_map(&summary_by_event, "Dr")?,
                l1_misses: Self::summary_from_map(&summary_by_event, "D1mr")?,
                l3_misses: Self::summary_from_map(&summary_by_event, "DLmr")?,
            },
            data_writes: CachegrindDataPoint {
                total: Self::summary_from_map(&summary_by_event, "Dw")?,
                l1_misses: Self::summary_from_map(&summary_by_event, "D1mw")?,
                l3_misses: Self::summary_from_map(&summary_by_event, "DLmw")?,
            },
        })
    }

    fn summary_from_map(map: &HashMap<&str, u64>, key: &str) -> Result<u64, ParseError> {
        map.get(key)
            .copied()
            .ok_or_else(|| format!("missing summary for event `{key}`").into())
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AccessSummary {
    pub instructions: u64,
    pub l1_hits: u64,
    pub l3_hits: u64,
    pub ram_accesses: u64,
}

impl AccessSummary {
    pub fn estimated_cycles(&self) -> u64 {
        // Uses Itamar Turner-Trauring's formula from https://pythonspeed.com/articles/consistent-benchmarking-in-ci/
        self.l1_hits + 5 * self.l3_hits + 35 * self.ram_accesses
    }
}

impl From<CachegrindSummary> for AccessSummary {
    fn from(summary: CachegrindSummary) -> Self {
        let ram_accesses = summary.instructions.l3_misses
            + summary.data_reads.l3_misses
            + summary.data_writes.l3_misses;
        let at_least_l3_hits = summary.instructions.l1_misses
            + summary.data_reads.l1_misses
            + summary.data_writes.l1_misses;
        let l3_hits = at_least_l3_hits - ram_accesses;
        let total_accesses =
            summary.instructions.total + summary.data_reads.total + summary.data_writes.total;
        let l1_hits = total_accesses - at_least_l3_hits;
        Self {
            instructions: summary.instructions.total,
            l1_hits,
            l3_hits,
            ram_accesses,
        }
    }
}

pub(crate) fn run_instrumented<T>(mut bench: impl FnMut() -> T) {
    crabgrind::cachegrind::start_instrumentation();
    let _output = bench();
    crabgrind::cachegrind::stop_instrumentation();
    // output is dropped outside the instrumented section
}
