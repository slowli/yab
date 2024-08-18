use std::{env, num, num::NonZeroUsize, process, process::Command};

use clap::Parser;

use crate::{bencher::BenchMode, reporter::Reporter, BenchmarkId};

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
    /// `{OUT}` will be replaced with the path to the output file.
    #[arg(
        long,
        alias = "cg",
        default_values_t = DEFAULT_CACHEGRIND_WRAPPER.iter().copied().map(str::to_owned)
    )]
    cachegrind_wrapper: Vec<String>,
    /// Target number of instructions for the benchmark warm-up. Note that this number may not be reached
    /// for very fast benchmarks.
    #[arg(long = "warm-up", default_value_t = 1_000_000)]
    pub warm_up_instructions: u64,
    /// Maximum number of iterations for a single benchmark.
    #[arg(long, default_value_t = 1_000)]
    pub max_iterations: u64,
    /// Base directory to put cachegrind outputs into. Will be created if absent.
    #[arg(long, default_value = "target/yab", env = "CACHEGRIND_OUT_DIR")]
    pub cachegrind_out_dir: String,
    /// Maximum number of benchmarks to run in parallel.
    #[arg(long, short = 'j', default_value_t = NonZeroUsize::new(num_cpus::get().max(1)).unwrap())]
    pub jobs: NonZeroUsize,

    /// List all benchmarks instead of running them.
    #[arg(long, conflicts_with = "print")]
    list: bool,
    /// Prints latest benchmark results without running benchmarks.
    #[arg(long, conflicts_with = "list")]
    print: bool,
    /// Match benchmark names exactly.
    #[arg(long)]
    exact: bool,
    /// Skip benchmarks whose names do not contain FILTER.
    #[arg(name = "FILTER")]
    filter: Option<String>,
}

impl BenchOptions {
    pub fn validate(&self, reporter: &mut Reporter) -> bool {
        if self.warm_up_instructions == 0 {
            reporter.report_fatal_error(&"`warm_up_instructions` must be positive");
            return false;
        }
        if self.max_iterations == 0 {
            reporter.report_fatal_error(&"`max_iterations` must be positive");
            return false;
        }
        true
    }

    pub fn mode(&self) -> BenchMode {
        if self.list {
            BenchMode::List
        } else if self.print {
            BenchMode::PrintResults
        } else if self.bench {
            BenchMode::Bench
        } else {
            BenchMode::Test
        }
    }

    pub fn should_run(&self, id: &BenchmarkId) -> bool {
        let id_string = id.to_string();
        if self.exact {
            self.filter
                .as_ref()
                .map_or(false, |filter| *filter == id_string)
        } else {
            self.filter
                .as_ref()
                .map_or(true, |filter| id_string.contains(filter))
        }
    }

    pub fn cachegrind_wrapper(&self, out_file: &str) -> Command {
        let mut command = Command::new(&self.cachegrind_wrapper[0]);
        command.args(&self.cachegrind_wrapper[1..]);
        command.arg(format!("--cachegrind-out-file={out_file}"));
        command
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
