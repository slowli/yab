use std::process::Command;

use clap::Parser;

use crate::{BenchMode, BenchmarkId};

#[derive(Debug, Parser)]
pub(crate) struct Options {
    /// Whether to run benchmarks as opposed to tests.
    #[arg(long, hide = true)]
    bench: bool,
    /// Instrument benchmarks with cachegrind instrumentation.
    #[arg(long, hide = true, conflicts_with_all = ["bench", "list"])]
    cachegrind_instrument: bool,
    #[arg(
        long,
        hide = true,
        default_value_t = 1,
        conflicts_with_all = ["bench", "list"],
        requires = "cachegrind_instrument"
    )]
    cachegrind_iterations: u64,

    /// Wrapper to call `cachegrind` as. Beware that changing params will likely render results not comparable.
    /// `{OUT}` will be replaced with the path to the output file.
    #[arg(
        long,
        default_values_t = [
            "setarch",
            "-R",
            "valgrind",
            "--tool=cachegrind",
            "--cache-sim=yes",
            "--instr-at-start=no",
            "--I1=32768,8,64",
            "--D1=32768,8,64",
            "--LL=8388608,16,64",
            "--cachegrind-out-file={OUT}",
        ].map(str::to_owned)
    )]
    cachegrind_wrapper: Vec<String>,
    /// Minimum instructions.
    #[arg(long, default_value_t = 1_000_000)]
    pub min_instructions: u64,
    /// Base directory to put cachegrind outputs into. Will be created if absent.
    #[arg(long, default_value = "target/yab", env = "CACHEGRIND_OUT_DIR")]
    pub cachegrind_out_dir: String,

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

impl Options {
    pub fn mode(&self) -> BenchMode {
        if self.cachegrind_instrument {
            BenchMode::Instrument(self.cachegrind_iterations)
        } else if self.list {
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
        for arg in &self.cachegrind_wrapper[1..] {
            if arg.contains("{OUT}") {
                command.arg(arg.replace("{OUT}", out_file));
            } else {
                command.arg(arg);
            }
        }
        command
    }
}
