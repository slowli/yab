use std::{
    env, fmt, fs,
    hash::{Hash, Hasher},
    panic,
    panic::Location,
    process,
};

use clap::Parser;

use self::{options::Options, reporter::Reporter};
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

#[derive(Debug, Clone, Copy)]
enum BenchMode {
    Test,
    Bench,
    List,
    PrintResults,
    Instrument,
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
                let out_path = format!("{}/{id}.cachegrind", self.options.cachegrind_out_dir);
                let old_summary = self.load_summary(&out_path);
                if old_summary.is_some() {
                    let backup_path = format!("{out_path}.old");
                    if let Err(err) = fs::copy(&out_path, &backup_path) {
                        let err =
                            format!("Failed backing up cachegrind output `{out_path}`: {err}");
                        self.reporter.report_warning(&err);
                    }
                }

                let command = self.options.cachegrind_wrapper(&out_path);
                let bench_reporter = self.reporter.report_bench(&id);
                let cachegrind_result =
                    cachegrind::spawn_instrumented(command, &out_path, &self.this_executable, &id);
                let summary = match cachegrind_result {
                    Ok(summary) => summary,
                    Err(err) => {
                        self.reporter.report_fatal_error(&err);
                        process::exit(1);
                    }
                };
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
                let out_path = format!("{}/{id}.cachegrind", self.options.cachegrind_out_dir);
                let Some(summary) = self.load_summary(&out_path) else {
                    self.reporter.report_bench_result(&id).no_data();
                    return self;
                };
                let backup_path = format!("{out_path}.old");
                let old_summary = self.load_summary(&backup_path);
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
            BenchMode::Instrument => {
                cachegrind::run_instrumented(bench_fn);
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
}
