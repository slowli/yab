//! [`Bencher`] and tightly related types.

use std::{env, fs, mem, panic, process, sync::Arc, thread, thread::JoinHandle};

use crate::{
    cachegrind,
    cachegrind::{CachegrindError, SpawnArgs},
    options::{BenchOptions, CachegrindOptions, Options},
    reporter::Reporter,
    utils::Semaphore,
    BenchmarkId, BenchmarkOutput, BenchmarkProcessor, CachegrindSummary, Capture,
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
    processor: Arc<dyn BenchmarkProcessor>,
    reporter: Reporter,
    this_executable: String,
    jobs_semaphore: Arc<Semaphore>,
    jobs: Vec<JoinHandle<()>>,
}

impl Drop for MainBencher {
    fn drop(&mut self) {
        for job in mem::take(&mut self.jobs) {
            job.join().expect("benchmarking failed");
        }
    }
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
            processor: Arc::new(()),
            reporter,
            this_executable: env::args().next().expect("no executable arg"),
            jobs_semaphore: Arc::new(Semaphore::new(options.jobs.get())),
            options,
            jobs: vec![],
        }
    }

    fn bench<T>(&mut self, id: BenchmarkId, mut bench_fn: impl FnMut(Capture) -> T) {
        if !self.options.should_run(&id) {
            return;
        }

        match self.mode {
            BenchMode::Test => {
                // Run the function once w/o instrumentation.
                let test_reporter = self.reporter.report_test(&id);
                if cfg!(panic = "unwind") {
                    let wrapped = panic::AssertUnwindSafe(move || drop(bench_fn(Capture::no_op())));
                    if panic::catch_unwind(wrapped).is_err() {
                        test_reporter.fail();
                        return;
                    }
                } else {
                    bench_fn(Capture::no_op());
                }
                test_reporter.ok();
            }
            BenchMode::Bench => {
                let executor = self.executor(id);
                let jobs_semaphore = self.jobs_semaphore.clone();
                self.jobs.push(thread::spawn(move || {
                    let _permit = jobs_semaphore.acquire_owned();
                    executor.run_benchmark();
                }));
            }
            BenchMode::List => {
                self.reporter.report_list_item(&id);
            }
            BenchMode::PrintResults => {
                self.executor(id).report_benchmark_result();
            }
        }
    }

    fn executor(&self, id: BenchmarkId) -> Executor {
        Executor {
            options: self.options.clone(),
            this_executable: self.this_executable.clone(),
            reporter: self.reporter.clone(),
            processor: self.processor.clone(),
            id,
        }
    }
}

#[derive(Debug)]
struct Executor {
    options: BenchOptions,
    this_executable: String,
    reporter: Reporter,
    processor: Arc<dyn BenchmarkProcessor>,
    id: BenchmarkId,
}

impl Executor {
    /// The workflow is as follows:
    ///
    /// 1. Run the benchmark function once to understand how many iterations are necessary for warm-up, `n`.
    /// 2. Run the *baseline* with `n + 1` iterations terminating after the setup on the last iteration.
    ///    I.e., the "timing" of this run is `(n + 1) * setup + n * bench + const`.
    /// 3. Run the full benchmark with `n + 1` iterations. The "timing" of this run is
    ///    `(n + 1) * setup + (n + 1) * bench + const`.
    /// 4. Subtract baseline stats from the full stats. The difference is equal to `bench`.
    fn run_benchmark(self) {
        let baseline_path = format!(
            "{}/{}.baseline.cachegrind",
            self.options.cachegrind_out_dir, self.id
        );
        let full_path = format!("{}/{}.cachegrind", self.options.cachegrind_out_dir, self.id);
        let old_baseline = self.load_and_backup_summary(&baseline_path);
        let old_summary = old_baseline.and_then(|baseline| {
            let full = self.load_and_backup_summary(&full_path)?;
            Some(full - baseline)
        });

        // Use `baseline_path` in case we won't run the baseline after calibration
        let command = self.options.cachegrind_wrapper(&baseline_path);
        let bench_reporter = self.reporter.report_bench(&self.id);
        let cachegrind_result = cachegrind::spawn_instrumented(SpawnArgs {
            command,
            out_path: &baseline_path,
            this_executable: &self.this_executable,
            id: &self.id,
            iterations: 2,
            is_baseline: true,
        });
        let summary = self.unwrap_summary(cachegrind_result);

        let estimated_iterations = (self.options.warm_up_instructions / summary.instructions.total)
            .clamp(1, self.options.max_iterations);
        let baseline_summary = if estimated_iterations == 1 {
            summary
        } else {
            // This will override calibration output, which is exactly what we need.
            let command = self.options.cachegrind_wrapper(&baseline_path);
            let cachegrind_result = cachegrind::spawn_instrumented(SpawnArgs {
                command,
                out_path: &baseline_path,
                this_executable: &self.this_executable,
                id: &self.id,
                iterations: estimated_iterations + 1,
                is_baseline: true,
            });
            let summary = self.unwrap_summary(cachegrind_result);
            bench_reporter.baseline(&summary);
            summary
        };

        let command = self.options.cachegrind_wrapper(&full_path);
        let cachegrind_result = cachegrind::spawn_instrumented(SpawnArgs {
            command,
            out_path: &full_path,
            this_executable: &self.this_executable,
            id: &self.id,
            iterations: estimated_iterations + 1,
            is_baseline: false,
        });
        let full_summary = self.unwrap_summary(cachegrind_result);
        let summary = full_summary - baseline_summary;

        bench_reporter.ok(summary, old_summary);
        self.processor.process_benchmark(
            &self.id,
            BenchmarkOutput {
                summary,
                old_summary,
            },
        );
    }

    fn report_benchmark_result(self) {
        let baseline_path = format!(
            "{}/{}.baseline.cachegrind",
            self.options.cachegrind_out_dir, self.id
        );
        let full_path = format!("{}/{}.cachegrind", self.options.cachegrind_out_dir, self.id);
        let Some(baseline) = self.load_summary(&baseline_path) else {
            self.reporter.report_bench_result(&self.id).no_data();
            return;
        };
        let Some(full) = self.load_summary(&full_path) else {
            self.reporter.report_bench_result(&self.id).no_data();
            return;
        };
        let summary = full - baseline;

        let old_baseline_path = format!("{baseline_path}.old");
        let old_full_path = format!("{full_path}.old");
        let old_baseline = self.load_summary(&old_baseline_path);
        let old_summary =
            old_baseline.and_then(|baseline| Some(self.load_summary(&old_full_path)? - baseline));

        self.reporter
            .report_bench_result(&self.id)
            .ok(summary, old_summary);
        self.processor.process_benchmark(
            &self.id,
            BenchmarkOutput {
                summary,
                old_summary,
            },
        );
    }

    fn load_summary(&self, path: &str) -> Option<CachegrindSummary> {
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

    fn load_and_backup_summary(&self, path: &str) -> Option<CachegrindSummary> {
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
        &self,
        result: Result<CachegrindSummary, CachegrindError>,
    ) -> CachegrindSummary {
        match result {
            Ok(summary) => summary,
            Err(err) => {
                self.reporter.report_fatal_error(&err);
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

/// Benchmarking manage, providing ability to define benchmarks.
#[derive(Debug)]
pub struct Bencher {
    inner: BencherInner,
}

/// Parses configuration options from the environment.
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
    #[doc(hidden)]
    pub fn with_processor(mut self, processor: impl BenchmarkProcessor) -> Self {
        if let BencherInner::Main(bencher) = &mut self.inner {
            bencher.processor = Arc::new(processor);
        }
        self
    }

    /// Benchmarks a single function. Dropping the output won't be included into the captured stats.
    #[track_caller]
    pub fn bench<T>(
        &mut self,
        id: impl Into<BenchmarkId>,
        mut bench_fn: impl FnMut() -> T,
    ) -> &mut Self {
        self.bench_inner(id.into(), move |capture| {
            capture.measure(&mut bench_fn); // dropping the output is not included into capture
        });
        self
    }

    /// Benchmarks a function with configurable capture interval. This allows set up before starting the capture
    /// and/or post-processing (e.g., assertions) after the capture.
    #[track_caller]
    pub fn bench_with_capture(
        &mut self,
        id: impl Into<BenchmarkId>,
        bench_fn: impl FnMut(Capture),
    ) -> &mut Self {
        self.bench_inner(id.into(), bench_fn);
        self
    }

    fn bench_inner(&mut self, id: BenchmarkId, bench_fn: impl FnMut(Capture)) {
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
