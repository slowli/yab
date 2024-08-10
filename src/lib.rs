use std::{
    env, fmt, fs,
    hash::{Hash, Hasher},
    panic,
    panic::Location,
    process,
};

use clap::Parser;

use self::{options::Options, reporter::Reporter};
use crate::{cachegrind::CachegrindError, reporter::BenchReporter};
pub use crate::{
    cachegrind::{AccessSummary, CachegrindSummary},
    output::{BenchmarkOutput, BenchmarkProcessor},
};

mod cachegrind;
mod options;
mod output;
mod reporter;

pub fn black_box<T>(dummy: T) -> T {
    unsafe {
        let ret = std::ptr::read_volatile(&dummy);
        std::mem::forget(dummy);
        ret
    }
}

#[derive(Debug, Clone)]
pub struct BenchmarkId {
    name: String,
    location: &'static Location<'static>,
    args: Option<String>,
}

impl PartialEq for BenchmarkId {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.args == other.args
    }
}

impl Eq for BenchmarkId {}

impl Hash for BenchmarkId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.args.hash(state);
    }
}

impl fmt::Display for BenchmarkId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(args) = &self.args {
            write!(formatter, "{}/{args}", self.name)
        } else {
            formatter.write_str(&self.name)
        }
    }
}

impl<S: Into<String>> From<S> for BenchmarkId {
    #[track_caller]
    fn from(name: S) -> Self {
        Self {
            name: name.into(),
            location: Location::caller(),
            args: None,
        }
    }
}

impl BenchmarkId {
    #[track_caller]
    pub fn new(name: impl Into<String>, args: impl fmt::Display) -> Self {
        Self {
            name: name.into(),
            location: Location::caller(),
            args: Some(args.to_string()),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

const MAX_ITERATIONS: u64 = 1_000;

#[derive(Debug, Clone, Copy)]
enum BenchMode {
    Test,
    Bench,
    List,
    PrintResults,
    Instrument(u64),
}

#[derive(Debug)]
pub struct Bencher {
    options: Options,
    mode: BenchMode,
    processor: Box<dyn BenchmarkProcessor>,
    reporter: Reporter,
    this_executable: String,
}

impl Default for Bencher {
    fn default() -> Self {
        let options = Options::parse();
        let mode = options.mode();
        if matches!(mode, BenchMode::Bench) {
            if let Err(err) = cachegrind::check() {
                eprintln!("{err}");
                process::exit(1);
            }
        }

        Self {
            mode,
            options,
            processor: Box::new(()),
            reporter: Reporter::default(),
            this_executable: env::args().next().expect("no executable arg"),
        }
    }
}

impl Bencher {
    pub fn with_processor(mut self, processor: impl BenchmarkProcessor + 'static) -> Self {
        self.processor = Box::new(processor);
        self
    }

    #[track_caller]
    pub fn bench_function<T>(
        &mut self,
        id: impl Into<BenchmarkId>,
        mut bench_fn: impl FnMut() -> T,
    ) -> &mut Self {
        let id = id.into();
        if !self.options.should_run(&id) {
            return self;
        }

        match self.mode {
            BenchMode::Test => {
                // Run the function once w/o instrumentation.
                let test_reporter = self.reporter.report_test(&id);
                if cfg!(panic = "unwind") {
                    let wrapped = panic::AssertUnwindSafe(move || drop(bench_fn()));
                    if panic::catch_unwind(wrapped).is_err() {
                        test_reporter.fail();
                        return self;
                    }
                } else {
                    bench_fn();
                }
                test_reporter.ok();
            }
            BenchMode::Bench => {
                let baseline_path = format!(
                    "{}/{id}.baseline.cachegrind",
                    self.options.cachegrind_out_dir
                );
                let full_path = format!("{}/{id}.cachegrind", self.options.cachegrind_out_dir);
                let old_baseline = self.load_and_backup_summary(&baseline_path);
                let old_summary = old_baseline.and_then(|baseline| {
                    let full = self.load_and_backup_summary(&full_path)?;
                    Some(full - baseline)
                });

                // Use `baseline_path` in case we won't run the baseline after calibration
                let command = self.options.cachegrind_wrapper(&baseline_path);
                let mut bench_reporter = self.reporter.report_bench(&id);
                let cachegrind_result = cachegrind::spawn_instrumented(
                    command,
                    &baseline_path,
                    &self.this_executable,
                    &id,
                    1,
                );
                let summary = Self::unwrap_summary(cachegrind_result, &mut bench_reporter);

                let estimated_iterations = (self.options.min_instructions
                    / summary.instructions.total)
                    .clamp(1, MAX_ITERATIONS);
                bench_reporter.calibration(&summary, estimated_iterations);
                let baseline_summary = if estimated_iterations == 1 {
                    summary
                } else {
                    // This will override calibration output, which is exactly what we need.
                    let command = self.options.cachegrind_wrapper(&baseline_path);
                    let cachegrind_result = cachegrind::spawn_instrumented(
                        command,
                        &baseline_path,
                        &self.this_executable,
                        &id,
                        estimated_iterations,
                    );
                    let summary = Self::unwrap_summary(cachegrind_result, &mut bench_reporter);
                    bench_reporter.baseline(&summary);
                    summary
                };

                let command = self.options.cachegrind_wrapper(&full_path);
                let cachegrind_result = cachegrind::spawn_instrumented(
                    command,
                    &full_path,
                    &self.this_executable,
                    &id,
                    estimated_iterations + 1,
                );
                let full_summary = Self::unwrap_summary(cachegrind_result, &mut bench_reporter);
                let summary = full_summary - baseline_summary;

                bench_reporter.ok(summary, old_summary);
                self.processor.process_benchmark(
                    &id,
                    BenchmarkOutput {
                        summary,
                        old_summary,
                    },
                );
            }
            BenchMode::List => {
                self.reporter.report_list_item(&id);
            }
            BenchMode::PrintResults => {
                let baseline_path = format!(
                    "{}/{id}.baseline.cachegrind",
                    self.options.cachegrind_out_dir
                );
                let full_path = format!("{}/{id}.cachegrind", self.options.cachegrind_out_dir);
                let Some(baseline) = self.load_summary(&baseline_path) else {
                    self.reporter.report_bench_result(&id).no_data();
                    return self;
                };
                let Some(full) = self.load_summary(&full_path) else {
                    self.reporter.report_bench_result(&id).no_data();
                    return self;
                };
                let summary = full - baseline;

                let old_baseline_path = format!("{baseline_path}.old");
                let old_full_path = format!("{full_path}.old");
                let old_baseline = self.load_summary(&old_baseline_path);
                let old_summary = old_baseline
                    .and_then(|baseline| Some(self.load_summary(&old_full_path)? - baseline));

                self.reporter
                    .report_bench_result(&id)
                    .ok(summary, old_summary);
                self.processor.process_benchmark(
                    &id,
                    BenchmarkOutput {
                        summary,
                        old_summary,
                    },
                );
            }
            BenchMode::Instrument(iterations) => {
                cachegrind::run_instrumented(bench_fn, iterations);
            }
        }
        self
    }

    fn load_summary(&mut self, path: &str) -> Option<CachegrindSummary> {
        fs::File::open(path)
            .ok()
            .and_then(|file| match CachegrindSummary::new(file, path) {
                Ok(summary) => Some(summary),
                Err(err) => {
                    self.reporter.report_warning(&err);
                    None
                }
            })
    }

    fn load_and_backup_summary(&mut self, path: &str) -> Option<CachegrindSummary> {
        let summary = self.load_summary(path);
        if summary.is_some() {
            let backup_path = format!("{path}.old");
            if let Err(err) = fs::copy(path, &backup_path) {
                let err = format!("Failed backing up cachegrind baseline `{path}`: {err}");
                self.reporter.report_warning(&err);
            }
        }
        summary
    }

    fn unwrap_summary(
        result: Result<CachegrindSummary, CachegrindError>,
        reporter: &mut BenchReporter<'_>,
    ) -> CachegrindSummary {
        match result {
            Ok(summary) => summary,
            Err(err) => {
                reporter.fatal(&err);
                process::exit(1);
            }
        }
    }
}
