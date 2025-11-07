use std::{
    env,
    ffi::OsString,
    io,
    io::IsTerminal,
    num,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process,
    process::Command,
};

use clap::{ColorChoice, Parser};
use regex::Regex;

use crate::{
    bencher::BenchMode,
    reporter::{PrintingReporter, Verbosity},
    BenchmarkId,
};

const DEFAULT_CACHEGRIND_WRAPPER: &[&str] = &[
    "setarch",
    "-R",
    "valgrind",
    "--tool=cachegrind",
    "--cache-sim=yes",
    #[cfg(feature = "instrumentation")]
    "--instr-at-start=no",
    "--I1=32768,8,64",
    "--D1=32768,8,64",
    "--LL=8388608,16,64",
];

#[allow(clippy::struct_excessive_bools)] // fine for command-line args
#[derive(Debug, Clone, Parser)]
pub(crate) struct BenchOptions {
    /// Whether to run benchmarks as opposed to tests.
    #[arg(long, hide = true)]
    bench: bool,

    /// Wrapper to call `cachegrind` as. Beware that changing params will likely render results not comparable.
    #[arg(
        long,
        alias = "cg",
        env = "CACHEGRIND_WRAPPER",
        value_delimiter = ':',
        default_values_t = DEFAULT_CACHEGRIND_WRAPPER.iter().copied().map(str::to_owned)
    )]
    cachegrind_wrapper: Vec<String>,
    /// Target number of instructions for the benchmark warm-up. Note that this number may not be reached
    /// for very fast benchmarks.
    #[arg(long = "warm-up", default_value_t = 1_000_000, value_name = "INSTR")]
    pub warm_up_instructions: u64,
    /// Maximum number of iterations for a single benchmark.
    #[arg(long, default_value_t = 1_000, value_name = "ITER")]
    pub max_iterations: u64,
    /// Base directory to put cachegrind outputs into. Will be created if absent.
    #[arg(
        long,
        value_name = "PATH",
        default_value = "target/yab",
        env = "CACHEGRIND_OUT_DIR"
    )]
    pub cachegrind_out_dir: PathBuf,
    /// Maximum number of benchmarks to run in parallel.
    #[arg(
        long,
        short = 'j',
        env = "CACHEGRIND_JOBS",
        default_value_t = NonZeroUsize::new(num_cpus::get().max(1)).unwrap()
    )]
    pub jobs: NonZeroUsize,

    /// Sets coloring of the program output.
    #[arg(long, env = "COLOR", default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,
    /// Output detailed benchmarking information.
    #[arg(long)]
    pub verbose: bool,
    /// Output only basic benchmarking information.
    #[arg(long, short = 'q', conflicts_with = "verbose")]
    pub quiet: bool,
    /// Output stats breakdown by function.
    #[arg(long)]
    pub breakdown: bool,

    /// Saves the full results as a named baseline.
    #[arg(long, value_name = "BASELINE")]
    save_baseline: Option<PathBuf>,
    /// Compares results against the specified baseline.
    #[arg(long, value_name = "BASELINE")]
    baseline: Option<PathBuf>,

    /// List all benchmarks instead of running them.
    #[arg(long, conflicts_with = "print")]
    list: bool,
    /// Prints latest benchmark results without running benchmarks. If `BASELINE` is specified, prints
    /// the specified baseline instead.
    #[arg(long, value_name = "BASELINE", conflicts_with = "list")]
    #[allow(clippy::option_option)] // necessary for clap
    print: Option<Option<PathBuf>>,
    /// Match benchmark names exactly.
    #[arg(long)]
    exact: bool,
    /// Skip benchmarks whose names do not match FILTER (a regular expression).
    #[arg(name = "FILTER")]
    filter: Option<String>,
}

impl BenchOptions {
    pub fn validate(&self, reporter: &mut PrintingReporter) -> bool {
        reporter.report_debug(format_args!("Started benchmarking with options: {self:?}"));

        if self.warm_up_instructions == 0 {
            reporter.report_error(None, &"`warm_up_instructions` must be positive");
            return false;
        }
        if self.max_iterations == 0 {
            reporter.report_error(None, &"`max_iterations` must be positive");
            return false;
        }
        true
    }

    pub fn mode(&self) -> BenchMode {
        if self.list {
            BenchMode::List
        } else if self.print.is_some() {
            BenchMode::PrintResults
        } else if self.bench {
            BenchMode::Bench
        } else {
            BenchMode::Test
        }
    }

    pub fn styling(&self) -> bool {
        match self.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => io::stderr().is_terminal(),
        }
    }

    pub fn verbosity(&self) -> Verbosity {
        if self.quiet {
            Verbosity::Quiet
        } else if self.verbose {
            Verbosity::Verbose
        } else {
            Verbosity::Normal
        }
    }

    pub fn id_matcher(&self) -> Result<IdMatcher, regex::Error> {
        Ok(match &self.filter {
            None => IdMatcher::Any,
            Some(str) if self.exact => IdMatcher::Exact(str.clone()),
            Some(re) => IdMatcher::Regex(Regex::new(re)?),
        })
    }

    pub fn cachegrind_wrapper(&self, out_file: &Path) -> Command {
        let mut command = Command::new(&self.cachegrind_wrapper[0]);
        command.args(&self.cachegrind_wrapper[1..]);
        let mut out_file_arg = OsString::from("--cachegrind-out-file=");
        out_file_arg.push(out_file);
        command.arg(out_file_arg);
        command
    }

    pub fn save_baseline_path(&self) -> Option<PathBuf> {
        let path = self.save_baseline.as_ref()?;
        // If --save-baseline specifies an absolute path, it will completely overwrite the save dir,
        // just as needed
        Some(self.cachegrind_out_dir.join("_baselines").join(path))
    }

    pub fn baseline_path(&self) -> Option<PathBuf> {
        let path = self.baseline.as_ref()?;
        // If --save-baseline specifies an absolute path, it will completely overwrite the save dir,
        // just as needed
        Some(self.cachegrind_out_dir.join("_baselines").join(path))
    }

    pub fn has_print_baseline(&self) -> bool {
        matches!(&self.print, Some(Some(_)))
    }

    pub fn print_baseline_path(&self) -> Option<PathBuf> {
        let path = self.print.as_ref()?.as_ref()?;
        Some(self.cachegrind_out_dir.join("_baselines").join(path))
    }
}

#[derive(Debug, thiserror::Error)]
enum CachegrindOptionsError {
    #[error("too few args; should be used as `--cachegrind-instrument ITERS +|- ID")]
    TooFewArgs,
    #[error("failed parsing iterations (must be a positive integer): {0}")]
    Iterations(#[source] num::ParseIntError),
    #[error("failed parsing baseline flag")]
    IsBaseline,
}

#[derive(Debug)]
pub(crate) struct CachegrindOptions {
    pub iterations: u64,
    pub is_baseline: bool,
    pub id: String,
    // TODO: consider index?
}

impl CachegrindOptions {
    const MARKER: &'static str = "--cachegrind-instrument";

    fn new() -> Result<Option<Self>, CachegrindOptionsError> {
        Self::parse_args(env::args())
    }

    pub fn push_args(&self, command: &mut Command) {
        let is_baseline = if self.is_baseline { "+" } else { "-" };
        command.args([
            Self::MARKER,
            &self.iterations.to_string(),
            is_baseline,
            &self.id,
        ]);
    }

    fn parse_args(
        mut args: impl Iterator<Item = String>,
    ) -> Result<Option<Self>, CachegrindOptionsError> {
        args.next();
        if args.next().as_deref() != Some(Self::MARKER) {
            return Ok(None);
        }

        let iterations = args.next().ok_or(CachegrindOptionsError::TooFewArgs)?;
        let iterations: u64 = iterations
            .parse()
            .map_err(CachegrindOptionsError::Iterations)?;
        let is_baseline = args.next().ok_or(CachegrindOptionsError::TooFewArgs)?;
        let is_baseline = match is_baseline.as_str() {
            "+" => true,
            "-" => false,
            _ => return Err(CachegrindOptionsError::IsBaseline),
        };
        let id = args.next().ok_or(CachegrindOptionsError::TooFewArgs)?;
        Ok(Some(Self {
            iterations,
            is_baseline,
            id,
        }))
    }
}

#[derive(Debug)]
pub(crate) enum IdMatcher {
    Any,
    Exact(String),
    Regex(Regex),
}

impl IdMatcher {
    pub fn matches(&self, id: &BenchmarkId) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(s) => *s == id.to_string(),
            Self::Regex(regex) => regex.is_match(&id.to_string()),
        }
    }
}

#[derive(Debug)]
pub(crate) enum Options {
    Bench(BenchOptions),
    Cachegrind(CachegrindOptions),
}

impl Options {
    pub fn new() -> Self {
        match CachegrindOptions::new() {
            Err(err) => {
                eprintln!("Failed starting instrumented binary: {err}");
                process::exit(1);
            }
            Ok(Some(options)) => return Self::Cachegrind(options),
            Ok(None) => { /* continue */ }
        }

        let options = BenchOptions::parse();
        Self::Bench(options)
    }
}

#[cfg(test)]
mod tests {
    use std::iter;

    use assert_matches::assert_matches;

    use super::*;

    #[test]
    fn parsing_cachegrind_options() {
        let options = CachegrindOptions::parse_args(iter::empty());
        assert_matches!(options, Ok(None));
        let args = ["yab", "--bench", "fib"].map(str::to_owned).into_iter();
        let options = CachegrindOptions::parse_args(args);
        assert_matches!(options, Ok(None));

        let args = ["yab", "--cachegrind-instrument", "123", "+", "fib"]
            .map(str::to_owned)
            .into_iter();
        let options = CachegrindOptions::parse_args(args)
            .unwrap()
            .expect("no options");
        assert_eq!(options.iterations, 123);
        assert!(options.is_baseline);
        assert_eq!(options.id, "fib");
    }
}
