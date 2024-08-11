//! [`Bencher`] and tightly related types.

use std::{env, fs, panic, process};

use crate::{
    cachegrind,
    cachegrind::{CachegrindError, SpawnArgs},
    options::{BenchOptions, CachegrindOptions, Options},
    reporter::{BenchReporter, Reporter},
    BenchmarkId, BenchmarkOutput, BenchmarkProcessor, CachegrindSummary, Instrumentation,
};

#[derive(Debug, Clone, Copy)]
pub(crate) enum BenchMode {
    Test,
    Bench,
    List,
    PrintResults,
}

/// Bencher variant executing in the normal (not cachegrind-supervised) mode.
#[derive(Debug)]
struct MainBencher {
    options: BenchOptions,
    mode: BenchMode,
    processor: Box<dyn BenchmarkProcessor>,
    reporter: Reporter,
    this_executable: String,
}

impl MainBencher {
    fn new(options: BenchOptions) -> Self {
        let mut reporter = Reporter::default();
        if !options.validate(&mut reporter) {
            process::exit(1);
        }
        let mode = options.mode();
        if matches!(mode, BenchMode::Bench) {
            if let Err(err) = cachegrind::check() {
                reporter.report_fatal_error(&err);
                process::exit(1);
            }
        }

        Self {
            mode,
            options,
            processor: Box::new(()),
            reporter,
            this_executable: env::args().next().expect("no executable arg"),
        }
    }

    fn bench<T>(&mut self, id: BenchmarkId, mut bench_fn: impl FnMut(Instrumentation) -> T) {
        if !self.options.should_run(&id) {
            return;
        }

        match self.mode {
            BenchMode::Test => {
                // Run the function once w/o instrumentation.
                let test_reporter = self.reporter.report_test(&id);
                if cfg!(panic = "unwind") {
                    let wrapped =
                        panic::AssertUnwindSafe(move || drop(bench_fn(Instrumentation::no_op())));
                    if panic::catch_unwind(wrapped).is_err() {
                        test_reporter.fail();
                        return;
                    }
                } else {
                    bench_fn(Instrumentation::no_op());
                }
                test_reporter.ok();
            }
            BenchMode::Bench => {
                self.run_benchmark(id);
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
                    return;
                };
                let Some(full) = self.load_summary(&full_path) else {
                    self.reporter.report_bench_result(&id).no_data();
                    return;
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
        }
    }

    /// The workflow is as follows:
    ///
    /// 1. Run the benchmark function once to understand how many iterations are necessary for warm-up, `n`.
    /// 2. Run the *baseline* with `n + 1` iterations terminating after the setup on the last iteration.
    ///    I.e., the "timing" of this run is `(n + 1) * setup + n * bench + const`.
    /// 3. Run the full benchmark with `n + 1` iterations. The "timing" of this run is
    ///    `(n + 1) * setup + (n + 1) * bench + const`.
    /// 4. Subtract baseline stats from the full stats. The difference is equal to `bench`.
    fn run_benchmark(&mut self, id: BenchmarkId) {
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
        let cachegrind_result = cachegrind::spawn_instrumented(SpawnArgs {
            command,
            out_path: &baseline_path,
            this_executable: &self.this_executable,
            id: &id,
            iterations: 2,
            is_baseline: true,
        });
        let summary = Self::unwrap_summary(cachegrind_result, &mut bench_reporter);

        let estimated_iterations = (self.options.warm_up_instructions / summary.instructions.total)
            .clamp(1, self.options.max_iterations);
        bench_reporter.calibration(&summary, estimated_iterations);
        let baseline_summary = if estimated_iterations == 1 {
            summary
        } else {
            // This will override calibration output, which is exactly what we need.
            let command = self.options.cachegrind_wrapper(&baseline_path);
            let cachegrind_result = cachegrind::spawn_instrumented(SpawnArgs {
                command,
                out_path: &baseline_path,
                this_executable: &self.this_executable,
                id: &id,
                iterations: estimated_iterations + 1,
                is_baseline: true,
            });
            let summary = Self::unwrap_summary(cachegrind_result, &mut bench_reporter);
            bench_reporter.baseline(&summary);
            summary
        };

        let command = self.options.cachegrind_wrapper(&full_path);
        let cachegrind_result = cachegrind::spawn_instrumented(SpawnArgs {
            command,
            out_path: &full_path,
            this_executable: &self.this_executable,
            id: &id,
            iterations: estimated_iterations + 1,
            is_baseline: false,
        });
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

#[derive(Debug)]
enum BencherInner {
    Main(MainBencher),
    Cachegrind(CachegrindOptions),
}

#[derive(Debug)]
pub struct Bencher {
    inner: BencherInner,
}

impl Default for Bencher {
    fn default() -> Self {
        let inner = match Options::new() {
            Options::Bench(options) => BencherInner::Main(MainBencher::new(options)),
            Options::Cachegrind(options) => BencherInner::Cachegrind(options),
        };
        Self { inner }
    }
}

impl Bencher {
    pub fn with_processor(mut self, processor: impl BenchmarkProcessor + 'static) -> Self {
        if let BencherInner::Main(bencher) = &mut self.inner {
            bencher.processor = Box::new(processor);
        }
        self
    }

    #[track_caller]
    pub fn bench<T>(
        &mut self,
        id: impl Into<BenchmarkId>,
        mut bench_fn: impl FnMut() -> T,
    ) -> &mut Self {
        self.bench_inner(id.into(), move |instrumentation| {
            instrumentation.start();
            bench_fn()
        });
        self
    }

    #[track_caller]
    pub fn bench_with_setup<T>(
        &mut self,
        id: impl Into<BenchmarkId>,
        bench_fn: impl FnMut(Instrumentation) -> T,
    ) -> &mut Self {
        self.bench_inner(id.into(), bench_fn);
        self
    }

    fn bench_inner<T>(&mut self, id: BenchmarkId, bench_fn: impl FnMut(Instrumentation) -> T) {
        match &mut self.inner {
            BencherInner::Main(bencher) => {
                bencher.bench(id, bench_fn);
            }
            BencherInner::Cachegrind(options) => {
                if id != options.id.as_str() {
                    return;
                }
                cachegrind::run_instrumented(bench_fn, options.iterations, options.is_baseline);
            }
        }
    }
}
