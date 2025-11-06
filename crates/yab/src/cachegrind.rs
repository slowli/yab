//! `cachegrind`-related logic.

use std::{
    borrow::Cow,
    collections::HashMap,
    convert::Infallible,
    fmt, fs, io,
    io::BufRead,
    ops,
    path::{Path, PathBuf},
    process,
    process::{Command, ExitStatus},
    str::FromStr,
};

use serde::{Deserialize, Serialize};

use crate::{options::CachegrindOptions, BenchmarkId};

#[derive(Debug)]
pub(crate) struct ExecFailure {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl ExecFailure {
    fn new(output: &process::Output) -> Self {
        Self {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        }
    }
}

impl fmt::Display for ExecFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.status)?;
        if !self.stdout.is_empty() {
            writeln!(formatter, "\n---- cachegrind stdout ----\n{}", self.stdout)?;
        }
        if !self.stderr.is_empty() {
            writeln!(formatter, "\n---- cachegrind stderr ----\n{}", self.stderr)?;
        }
        Ok(())
    }
}

impl std::error::Error for ExecFailure {}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CachegrindError {
    #[error("I/O error executing cachegrind: {0}")]
    Exec(#[source] io::Error),
    #[error("cachegrind exited abnormally: {0}")]
    ExecFailure(#[from] ExecFailure),
    #[error(
        "Unable to get `cachegrind` version. Please make sure that `valgrind` is installed \
         and is on PATH"
    )]
    NoCachegrind,

    #[error("I/O error creating output directory `{path}`: {error}", path = path.display())]
    CreateOutputDir {
        path: PathBuf,
        #[source]
        error: io::Error,
    },
    #[error("I/O error reading cachegrind output at `{path}`: {error}", path = out_path.display())]
    Read {
        out_path: PathBuf,
        #[source]
        error: io::Error,
    },
    #[error("Failed parsing cachegrind output at `{path}`: {message}", path = out_path.display())]
    Parse {
        out_path: PathBuf,
        message: Cow<'static, str>,
    },
}

#[derive(Debug)]
enum ParseError {
    Custom(Cow<'static, str>),
    Io(io::Error),
}

impl ParseError {
    fn generalize(self, out_path: PathBuf) -> CachegrindError {
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

pub(crate) fn check() -> Result<String, CachegrindError> {
    let output = Command::new("valgrind")
        .args(["--tool=cachegrind", "--version"])
        .output()
        .map_err(CachegrindError::Exec)?;
    if !output.status.success() {
        return Err(CachegrindError::NoCachegrind);
    }
    let version = String::from_utf8(output.stdout)
        .map_err(|err| CachegrindError::Exec(io::Error::other(err)))?;
    Ok(version.trim().to_owned())
}

#[derive(Debug)]
pub(crate) struct SpawnArgs<'a> {
    pub command: Command,
    pub out_path: &'a Path,
    pub this_executable: &'a str,
    pub id: &'a BenchmarkId,
    pub iterations: u64,
    pub is_baseline: bool,
}

pub(crate) fn spawn_instrumented(args: SpawnArgs) -> Result<CachegrindOutput, CachegrindError> {
    let SpawnArgs {
        mut command,
        out_path,
        this_executable,
        id,
        iterations,
        is_baseline,
    } = args;

    if let Some(parent_dir) = out_path.parent() {
        fs::create_dir_all(parent_dir).map_err(|error| CachegrindError::CreateOutputDir {
            path: parent_dir.to_owned(),
            error,
        })?;
    }

    command.arg(this_executable);
    let options = CachegrindOptions {
        iterations,
        is_baseline,
        id: id.to_string(),
    };
    options.push_args(&mut command);

    let output = command.output().map_err(CachegrindError::Exec)?;
    if !output.status.success() {
        return Err(ExecFailure::new(&output).into());
    }

    let out = fs::File::open(out_path).map_err(|error| CachegrindError::Read {
        out_path: out_path.to_owned(),
        error,
    })?;
    CachegrindOutput::read(io::BufReader::new(out))
        .map_err(|err| err.generalize(out_path.to_owned()))
}

/// Information about a particular type of operations (instruction reads, data reads / writes).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct CachegrindDataPoint {
    /// Total number of operations performed.
    pub total: u64,
    /// Number of operations that have missed L1 cache.
    pub l1_misses: u64,
    /// Number of operations that have missed L2/L3 caches.
    pub l3_misses: u64,
}

impl CachegrindDataPoint {
    pub(crate) fn l1_hits(&self) -> u64 {
        self.total - self.l1_misses
    }

    pub(crate) fn l3_hits(&self) -> u64 {
        self.l1_misses - self.l3_misses
    }
}

impl ops::Add for CachegrindDataPoint {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            total: self.total + rhs.total,
            l1_misses: self.l1_misses + rhs.l1_misses,
            l3_misses: self.l3_misses + rhs.l3_misses,
        }
    }
}

impl ops::Sub for CachegrindDataPoint {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self {
            total: self.total.saturating_sub(rhs.total),
            l1_misses: self.l1_misses.saturating_sub(rhs.l1_misses),
            l3_misses: self.l3_misses.saturating_sub(rhs.l3_misses),
        }
    }
}

impl ops::Mul<u64> for CachegrindDataPoint {
    type Output = Self;

    fn mul(self, rhs: u64) -> Self::Output {
        Self {
            total: self.total * rhs,
            l1_misses: self.l1_misses * rhs,
            l3_misses: self.l3_misses * rhs,
        }
    }
}

/// Full `cachegrind` stats including cache simulation.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct FullCachegrindStats {
    /// Instruction-related statistics.
    pub instructions: CachegrindDataPoint,
    /// Statistics related to data reads.
    pub data_reads: CachegrindDataPoint,
    /// Statistics related to data writes.
    pub data_writes: CachegrindDataPoint,
}

impl FullCachegrindStats {
    fn read(summary_by_event: &HashMap<&str, u64>) -> Result<Self, ParseError> {
        Ok(Self {
            instructions: CachegrindDataPoint {
                total: summary_from_map(summary_by_event, "Ir")?,
                l1_misses: summary_from_map(summary_by_event, "I1mr")?,
                l3_misses: summary_from_map(summary_by_event, "ILmr")?,
            },
            data_reads: CachegrindDataPoint {
                total: summary_from_map(summary_by_event, "Dr")?,
                l1_misses: summary_from_map(summary_by_event, "D1mr")?,
                l3_misses: summary_from_map(summary_by_event, "DLmr")?,
            },
            data_writes: CachegrindDataPoint {
                total: summary_from_map(summary_by_event, "Dw")?,
                l1_misses: summary_from_map(summary_by_event, "D1mw")?,
                l3_misses: summary_from_map(summary_by_event, "DLmw")?,
            },
        })
    }

    fn is_zero(&self) -> bool {
        self.instructions.total == 0 && self.data_reads.total == 0 && self.data_writes.total == 0
    }
}

fn summary_from_map(map: &HashMap<&str, u64>, key: &str) -> Result<u64, ParseError> {
    map.get(key)
        .copied()
        .ok_or_else(|| format!("missing summary for event `{key}`").into())
}

impl ops::Add for FullCachegrindStats {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            instructions: self.instructions + rhs.instructions,
            data_reads: self.data_reads + rhs.data_reads,
            data_writes: self.data_writes + rhs.data_writes,
        }
    }
}

impl ops::Sub for FullCachegrindStats {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self {
            instructions: self.instructions - rhs.instructions,
            data_reads: self.data_reads - rhs.data_reads,
            data_writes: self.data_writes - rhs.data_writes,
        }
    }
}

impl ops::Mul<u64> for FullCachegrindStats {
    type Output = Self;

    fn mul(self, rhs: u64) -> Self::Output {
        Self {
            instructions: self.instructions * rhs,
            data_reads: self.data_reads * rhs,
            data_writes: self.data_reads * rhs,
        }
    }
}

/// Raw summary output produced by `cachegrind`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum CachegrindStats {
    /// Stats produced by `cachegrind` with disabled cache simulation.
    #[non_exhaustive]
    Simple {
        /// Total number of executed instructions.
        instructions: u64,
    },
    /// Full stats including cache simulation.
    Full(FullCachegrindStats),
}

impl Default for CachegrindStats {
    fn default() -> Self {
        Self::Full(FullCachegrindStats::default())
    }
}

impl ops::Add for CachegrindStats {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Self::Full(lhs), Self::Full(rhs)) => Self::Full(lhs + rhs),
            _ => Self::Simple {
                instructions: self.total_instructions() + rhs.total_instructions(),
            },
        }
    }
}

impl ops::AddAssign for CachegrindStats {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

/// Uses saturated subtraction for all primitive `u64` values.
impl ops::Sub for CachegrindStats {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Self::Full(lhs), Self::Full(rhs)) => Self::Full(lhs - rhs),
            _ => Self::Simple {
                instructions: self
                    .total_instructions()
                    .saturating_sub(rhs.total_instructions()),
            },
        }
    }
}

impl CachegrindStats {
    /// Returns full stats if they are available.
    pub fn as_full(&self) -> Option<&FullCachegrindStats> {
        match self {
            Self::Full(stats) => Some(stats),
            Self::Simple { .. } => None,
        }
    }

    /// Gets the total number of executed instructions.
    pub fn total_instructions(&self) -> u64 {
        match self {
            Self::Simple { instructions } => *instructions,
            Self::Full(stats) => stats.instructions.total,
        }
    }

    fn is_zero(&self) -> bool {
        match self {
            Self::Simple { instructions } => *instructions == 0,
            Self::Full(stats) => stats.is_zero(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CachegrindOutput {
    pub summary: CachegrindStats,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub breakdown: HashMap<CachegrindFunction, CachegrindStats>,
}

impl CachegrindOutput {
    pub(crate) fn new(file: fs::File, path: &Path) -> Result<Self, CachegrindError> {
        let reader = io::BufReader::new(file);
        Self::read(reader).map_err(|err| err.generalize(path.to_owned()))
    }

    fn read(reader: impl BufRead) -> Result<Self, ParseError> {
        let mut events = None;
        let mut summary_line = None;

        let mut filename = None;
        let mut function_name = None;
        let mut breakdown = HashMap::new();
        for line in reader.lines() {
            let line = line?;
            if let Some(events_line) = line.strip_prefix("events:") {
                if events.is_some() {
                    return Err("events are redefined".into());
                }
                events = Some(
                    events_line
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect::<Vec<_>>(),
                );
            } else if let Some(summary) = line.strip_prefix("summary:") {
                if summary_line.is_some() {
                    return Err("summary is redefined".into());
                }
                summary_line = Some(summary.to_owned());
                break;
            } else if let Some(file) = line.strip_prefix("fl=") {
                filename = (file != "???").then(|| file.trim().to_owned());
            } else if let Some(name) = line.strip_prefix("fn=") {
                function_name = Some(name.to_owned());
            } else if let (Some(events), Some(function_name)) = (&events, &function_name) {
                let numbers: Vec<_> = line.split_whitespace().collect();
                if numbers.len() != events.len() + 1 {
                    return Err("mismatch between events and stats".into());
                }

                let summary_by_event: Result<HashMap<_, _>, ParseError> = events
                    .iter()
                    .zip(&numbers[1..])
                    .map(|(event, s)| {
                        let stat = s
                            .parse::<u64>()
                            .map_err(|_| format!("{event} stat is not an u64: {s}"))?;
                        Ok((event.as_str(), stat))
                    })
                    .collect();
                let summary_by_event = summary_by_event?;
                let stats = if summary_by_event.len() == 1 {
                    let instructions = summary_from_map(&summary_by_event, "Ir")?;
                    CachegrindStats::Simple { instructions }
                } else {
                    CachegrindStats::Full(FullCachegrindStats::read(&summary_by_event)?)
                };

                let function = CachegrindFunction {
                    filename: filename.clone(),
                    name: function_name.clone(),
                };
                *breakdown.entry(function).or_default() += stats;
            }
        }

        let events = events.ok_or("no events")?;
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

        let summary_by_event: HashMap<_, _> =
            events.iter().map(String::as_str).zip(summary).collect();
        let stats = if summary_by_event.len() == 1 {
            let instructions = summary_from_map(&summary_by_event, "Ir")?;
            CachegrindStats::Simple { instructions }
        } else {
            CachegrindStats::Full(FullCachegrindStats::read(&summary_by_event)?)
        };
        Ok(Self {
            summary: stats,
            breakdown,
        })
    }
}

impl ops::Sub for CachegrindOutput {
    type Output = Self;

    fn sub(self, mut rhs: Self) -> Self::Output {
        let breakdown_diff = self.breakdown.into_iter().filter_map(|(function, stats)| {
            let diff = if let Some(rhs_stats) = rhs.breakdown.remove(&function) {
                stats - rhs_stats
            } else {
                stats
            };
            (!diff.is_zero()).then_some((function, diff))
        });
        Self {
            summary: self.summary - rhs.summary,
            breakdown: breakdown_diff.collect(),
        }
    }
}

/// High-level memory access stats summarized from [`CachegrindStats`].
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct AccessSummary {
    /// Total number of instructions executed.
    pub instructions: u64,
    /// Total number of L1 cache hits (including instruction reads, data reads and data writes).
    pub l1_hits: u64,
    /// Total number of L2 / L3 cache hits (including instruction reads, data reads and data writes).
    pub l3_hits: u64,
    /// Total number of RAM accesses.
    pub ram_accesses: u64,
}

impl AccessSummary {
    /// Returns the estimated number of CPU cycles using Itamar Turner-Trauring's [formula].
    ///
    /// [formula]: https://pythonspeed.com/articles/consistent-benchmarking-in-ci/
    pub fn estimated_cycles(&self) -> u64 {
        self.l1_hits + 5 * self.l3_hits + 35 * self.ram_accesses
    }
}

impl From<FullCachegrindStats> for AccessSummary {
    fn from(stats: FullCachegrindStats) -> Self {
        let ram_accesses =
            stats.instructions.l3_misses + stats.data_reads.l3_misses + stats.data_writes.l3_misses;
        let at_least_l3_hits =
            stats.instructions.l1_misses + stats.data_reads.l1_misses + stats.data_writes.l1_misses;
        let l3_hits = at_least_l3_hits - ram_accesses;
        let total_accesses =
            stats.instructions.total + stats.data_reads.total + stats.data_writes.total;
        let l1_hits = total_accesses - at_least_l3_hits;
        Self {
            instructions: stats.instructions.total,
            l1_hits,
            l3_hits,
            ram_accesses,
        }
    }
}

/// Function associated with captured cachegrind stats.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CachegrindFunction {
    filename: Option<String>,
    name: String,
}

impl fmt::Display for CachegrindFunction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.name)?;
        if let Some(filename) = &self.filename {
            write!(formatter, "@{filename}")?;
        }
        Ok(())
    }
}

impl FromStr for CachegrindFunction {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some((name, filename)) = s.rsplit_once('@') else {
            return Ok(Self::rust(s));
        };
        Ok(Self {
            filename: Some(filename.to_owned()),
            name: name.to_owned(),
        })
    }
}

impl CachegrindFunction {
    /// Creates a new Rust-like function.
    pub fn rust(name: impl Into<String>) -> Self {
        Self {
            filename: None,
            name: name.into(),
        }
    }

    /// Returns the name of this function.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the filename for this function. For Rust, this field may be empty.
    pub fn filename(&self) -> Option<&str> {
        self.filename.as_deref()
    }
}

pub(crate) fn run_instrumented<T>(
    mut bench: impl FnMut(Capture) -> T,
    iterations: u64,
    is_baseline: bool,
) {
    let mut outputs = Vec::with_capacity(usize::try_from(iterations).expect("too many iterations"));

    #[cfg(feature = "instrumentation")]
    crabgrind::cachegrind::start_instrumentation();

    for i in 1..=iterations {
        let instrumentation = Capture {
            behavior: crate::black_box(match (i == iterations, is_baseline) {
                (false, _) => CaptureBehavior::NoOp,
                (true, true) => CaptureBehavior::TerminateOnStart,
                (true, false) => CaptureBehavior::TerminateOnEnd,
            }),
        };
        outputs.push(crate::black_box(bench(instrumentation)));
    }

    // Test outputs are intentionally never dropped
    #[cfg(feature = "instrumentation")]
    crabgrind::cachegrind::stop_instrumentation();
    process::exit(0);
}

#[derive(Debug)]
enum CaptureBehavior {
    NoOp,
    TerminateOnStart,
    TerminateOnEnd,
}

/// Manager of capturing benchmarking stats provided to closures in
/// [`Bencher::bench_with_capture()`](crate::Bencher::bench_with_capture()).
#[derive(Debug)]
#[must_use = "should be `start`ed"]
pub struct Capture {
    behavior: CaptureBehavior,
}

impl Capture {
    pub(crate) const fn no_op() -> Self {
        Self {
            behavior: CaptureBehavior::NoOp,
        }
    }

    /// Starts capturing stats.
    pub fn start(self) -> CaptureGuard {
        match crate::black_box(self.behavior) {
            CaptureBehavior::NoOp => CaptureGuard { terminate: false },
            CaptureBehavior::TerminateOnStart => {
                #[cfg(feature = "instrumentation")]
                crabgrind::cachegrind::stop_instrumentation();
                process::exit(0);
            }
            CaptureBehavior::TerminateOnEnd => CaptureGuard { terminate: true },
        }
    }

    /// Captures stats inside the provided closure (**not** including dropping its output).
    /// The output is wrapped in a [`black_box`](crate::black_box).
    #[inline]
    pub fn measure<T>(self, action: impl FnOnce() -> T) -> T {
        let _guard = self.start();
        crate::black_box(action())
    }
}

/// Guard returned by [`Capture::start()`]. When it is dropped, capturing stops.
#[must_use = "will stop capturing stats on drop"]
#[derive(Debug)]
pub struct CaptureGuard {
    terminate: bool,
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        if crate::black_box(self.terminate) {
            #[cfg(feature = "instrumentation")]
            crabgrind::cachegrind::stop_instrumentation();
            process::exit(0);
        }
    }
}

mod serde_helpers {
    use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

    use super::CachegrindFunction;

    impl Serialize for CachegrindFunction {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(&self.to_string())
        }
    }

    impl<'de> Deserialize<'de> for CachegrindFunction {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let raw = String::deserialize(deserializer)?;
            raw.parse().map_err(de::Error::custom)
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;

    #[test]
    fn parsing_basic_cachegrind_output() {
        let output = "\
            events: Ir\n\
            summary: 1234";
        let output = CachegrindOutput::read(output.as_bytes()).unwrap();
        assert_matches!(
            output.summary,
            CachegrindStats::Simple { instructions } if instructions == 1_234
        );
    }

    #[test]
    fn parsing_full_cachegrind_output() {
        let output = "\
            events: Ir I1mr ILmr Dr D1mr DLmr Dw D1mw DLmw \n\
            fn=<alloc::string::String as core::fmt::Write>::write_str\n\
            0 99 3 3 30 0 0 24 0 0\n\
            fn=<alloc::sync::Arc<T> as core::default::Default>::default\n\
            0 51 5 5 18 1 0 21 0 0\n\
            summary: 662469 1899 1843 143129 3638 2694 89043 1330 1210\n
        ";
        let output = CachegrindOutput::read(output.as_bytes()).unwrap();
        let stats = output.summary.as_full().unwrap();
        assert_full_stats(stats);

        let breakdown = output.breakdown;
        assert_eq!(breakdown.len(), 2);
        let fn1 =
            CachegrindFunction::rust("<alloc::string::String as core::fmt::Write>::write_str");
        let fn1_stats = breakdown[&fn1].as_full().unwrap();
        assert_eq!(fn1_stats.instructions.total, 99);
        assert_eq!(fn1_stats.data_reads.total, 30);
        assert_eq!(fn1_stats.data_writes.total, 24);

        let fn2 =
            CachegrindFunction::rust("<alloc::sync::Arc<T> as core::default::Default>::default");
        let fn2_stats = breakdown[&fn2].as_full().unwrap();
        assert_eq!(fn2_stats.instructions.total, 51);
        assert_eq!(fn2_stats.data_reads.total, 18);
        assert_eq!(fn2_stats.data_writes.total, 21);
    }

    fn assert_full_stats(stats: &FullCachegrindStats) {
        assert_eq!(stats.instructions.total, 662_469);
        assert_eq!(stats.instructions.l1_misses, 1_899);
        assert_eq!(stats.instructions.l3_misses, 1_843);
        assert_eq!(stats.data_reads.total, 143_129);
        assert_eq!(stats.data_reads.l1_misses, 3_638);
        assert_eq!(stats.data_reads.l3_misses, 2_694);
        assert_eq!(stats.data_writes.total, 89_043);
        assert_eq!(stats.data_writes.l1_misses, 1_330);
        assert_eq!(stats.data_writes.l3_misses, 1_210);
    }

    #[test]
    fn serializing_stats() {
        let json = serde_json::json!({
            "instructions": 1_234,
        });
        let stats: CachegrindStats = serde_json::from_value(json.clone()).unwrap();
        assert_matches!(
            stats,
            CachegrindStats::Simple { instructions } if instructions == 1_234
        );
        assert_eq!(serde_json::to_value(stats).unwrap(), json);

        let json = serde_json::json!({
            "instructions": {
                "total": 662_469,
                "l1_misses": 1_899,
                "l3_misses": 1_843,
            },
            "data_reads": {
                "total": 143_129,
                "l1_misses": 3_638,
                "l3_misses": 2_694,
            },
            "data_writes": {
                "total": 89_043,
                "l1_misses": 1_330,
                "l3_misses": 1_210,
            },
        });
        let stats: CachegrindStats = serde_json::from_value(json.clone()).unwrap();
        assert_full_stats(stats.as_full().unwrap());
        assert_eq!(serde_json::to_value(stats).unwrap(), json);
    }

    #[test]
    fn parsing_function() {
        let s = "<alloc::sync::Arc<T> as core::default::Default>::default";
        let function = CachegrindFunction::rust(s);
        assert_eq!(function.to_string(), s);
        let restored: CachegrindFunction = s.parse().unwrap();
        assert_eq!(restored, function);

        let with_file = "<alloc::sync::Arc<T> as core::default::Default>::default@path/to/file.rs";
        let restored: CachegrindFunction = with_file.parse().unwrap();
        assert_eq!(restored.filename.unwrap(), "path/to/file.rs");
        assert_eq!(restored.name, s);
    }
}
