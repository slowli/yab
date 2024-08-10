use std::{env, fs, panic, process};

use clap::Parser;

use self::{options::Options, reporter::Reporter};
use crate::cachegrind::CachegrindSummary;

mod cachegrind;
mod options;
mod reporter;

pub fn black_box<T>(dummy: T) -> T {
    unsafe {
        let ret = std::ptr::read_volatile(&dummy);
        std::mem::forget(dummy);
        ret
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
    reporter: Reporter,
    this_executable: String,
}

impl Default for Bencher {
    fn default() -> Self {
        Self::new()
    }
}

impl Bencher {
    fn new() -> Self {
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
            reporter: Reporter::default(),
            this_executable: env::args().next().expect("no executable arg"),
        }
    }

    pub fn bench_function<T>(
        &mut self,
        name: &str, // FIXME
        mut bench_fn: impl FnMut() -> T,
    ) -> &mut Self {
        if !self.options.should_run(name) {
            return self;
        }

        match self.mode {
            BenchMode::Test => {
                // Run the function once w/o instrumentation.
                let test_reporter = self.reporter.report_test(name);
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
                let out_path = format!("{}/{name}.cachegrind", self.options.cachegrind_out_dir);
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
                let bench_reporter = self.reporter.report_bench(name);
                let cachegrind_result =
                    cachegrind::spawn_instrumented(command, &out_path, &self.this_executable, name);
                let summary = match cachegrind_result {
                    Ok(summary) => summary,
                    Err(err) => {
                        self.reporter.report_fatal_error(&err);
                        process::exit(1);
                    }
                };
                bench_reporter.ok(summary, old_summary);
            }
            BenchMode::List => {
                self.reporter.report_list_item(name);
            }
            BenchMode::PrintResults => {
                let out_path = format!("{}/{name}.cachegrind", self.options.cachegrind_out_dir);
                let Some(summary) = self.load_summary(&out_path) else {
                    self.reporter.report_bench_result(name).no_data();
                    return self;
                };
                let backup_path = format!("{out_path}.old");
                let old_summary = self.load_summary(&backup_path);
                self.reporter
                    .report_bench_result(name)
                    .ok(summary, old_summary);
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
