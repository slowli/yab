//! [`Bencher`] and tightly related types.

use std::{env, fs, mem, panic, process, sync::Arc, thread, thread::JoinHandle};

use crate::{
    cachegrind,
    cachegrind::SpawnArgs,
    options::{BenchOptions, CachegrindOptions, IdMatcher, Options},
    reporter::{BenchmarkOutput, BenchmarkReporter, PrintingReporter, Reporter, SeqReporter},
    utils::Semaphore,
    BenchmarkId, CachegrindStats, Capture,
};

/// Mode in which the bencher is currently executing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BenchMode {
    /// Testing the benchmark code. Enabled by running benchmarks via `cargo test`.
    Test,
    /// Collecting benchmark data (i.e., the main / default mode).
    Bench,
    /// Listing benchmark names. Enabled by specifying `--list` command-line arg.
    List,
    /// Printing benchmark results collected during previous runs. Enabled by specifying `--print` command-line arg.
    PrintResults,
}

/// Mode-specific data.
#[derive(Debug)]
enum BenchModeData {
    Test {
        should_fail: bool,
    },
    Bench {
        this_executable: String,
        jobs_semaphore: Arc<Semaphore>,
        jobs: Vec<JoinHandle<()>>,
    },
    List,
    PrintResults,
}

impl BenchModeData {
    fn new(options: &BenchOptions) -> Self {
        match options.mode() {
            BenchMode::Test => Self::Test { should_fail: false },
            BenchMode::Bench => Self::Bench {
                this_executable: env::args().next().expect("no executable arg"),
                jobs_semaphore: Arc::new(Semaphore::new(options.jobs.get())),
                jobs: vec![],
            },
            BenchMode::List => Self::List,
            BenchMode::PrintResults => Self::PrintResults,
        }
    }

    fn mode(&self) -> BenchMode {
        match self {
            Self::Test { .. } => BenchMode::Test,
            Self::Bench { .. } => BenchMode::Bench,
            Self::List => BenchMode::List,
            Self::PrintResults => BenchMode::PrintResults,
        }
    }
}

/// Bencher variant executing in the normal (not cachegrind-supervised) mode.
#[derive(Debug)]
struct MainBencher {
    options: BenchOptions,
    id_matcher: IdMatcher,
    mode: BenchModeData,
    reporter: SeqReporter,
}

impl Drop for MainBencher {
    fn drop(&mut self) {
        if thread::panicking() {
            return;
        }

        match &mut self.mode {
            BenchModeData::Bench { jobs, .. } => {
                for job in mem::take(jobs) {
                    if job.join().is_err() {
                        self.reporter
                            .error(&"At least one of benchmarking jobs failed");
                        break;
                    }
                }
            }
            BenchModeData::Test { should_fail } if *should_fail => {
                self.reporter.error(&"There were test failures");
                process::exit(1);
            }
            _ => { /* no special handling required */ }
        }
        mem::take(&mut self.reporter).ok_all();
    }
}

impl MainBencher {
    fn new(options: BenchOptions) -> Self {
        let mut reporter = PrintingReporter::new(options.styling(), options.verbosity());
        if !options.validate(&mut reporter) {
            process::exit(1);
        }
        let mode = BenchModeData::new(&options);
        if matches!(mode, BenchModeData::Bench { .. }) {
            match cachegrind::check() {
                Ok(version) => {
                    reporter.report_debug(format_args!("Using cachegrind with version {version}"));
                }
                Err(err) => {
                    reporter.report_error(None, &err);
                    process::exit(1);
                }
            }
        }

        let id_matcher = match options.id_matcher() {
            Ok(matcher) => matcher,
            Err(err) => {
                reporter.report_error(None, &err);
                process::exit(1);
            }
        };

        Self {
            options,
            id_matcher,
            mode,
            reporter: SeqReporter(vec![Box::new(reporter)]),
        }
    }

    fn bench<T>(&mut self, id: BenchmarkId, mut bench_fn: impl FnMut(Capture) -> T) {
        if !self.id_matcher.matches(&id) {
            return;
        }

        match &mut self.mode {
            BenchModeData::Test { should_fail } => {
                let test_reporter = self.reporter.new_test(&id);
                // Run the function once w/o instrumentation.
                if cfg!(panic = "unwind") {
                    let wrapped = panic::AssertUnwindSafe(move || drop(bench_fn(Capture::no_op())));
                    if let Err(err) = panic::catch_unwind(wrapped) {
                        test_reporter.fail(&err);
                        *should_fail = true;
                        return;
                    }
                } else {
                    bench_fn(Capture::no_op());
                }
                test_reporter.ok();
            }
            BenchModeData::Bench {
                jobs_semaphore,
                jobs,
                this_executable,
            } => {
                let executor = CachegrindRunner {
                    options: self.options.clone(),
                    this_executable: this_executable.to_owned(),
                    reporter: self.reporter.new_benchmark(&id),
                    id,
                };

                if jobs_semaphore.capacity() == 1 {
                    // Run the executor synchronously in order to have deterministic ordering
                    executor.run_benchmark();
                } else {
                    let jobs_semaphore = jobs_semaphore.clone();
                    jobs.push(thread::spawn(move || {
                        let _permit = jobs_semaphore.acquire_owned();
                        executor.run_benchmark();
                    }));
                }
            }
            BenchModeData::List => {
                PrintingReporter::report_list_item(&id);
            }
            BenchModeData::PrintResults => {
                let executor = CachegrindRunner {
                    options: self.options.clone(),
                    reporter: self.reporter.new_benchmark(&id),
                    // `this_executable` isn't used, so it's fine to set it to an empty string
                    this_executable: String::new(),
                    id,
                };
                executor.report_benchmark_result();
            }
        }
    }
}

/// Runner for a single benchmark.
#[derive(Debug)]
struct CachegrindRunner {
    options: BenchOptions,
    this_executable: String,
    reporter: Box<dyn BenchmarkReporter>,
    id: BenchmarkId,
}

macro_rules! unwrap_summary {
    ($events:expr, $result:expr) => {
        match $result {
            Ok(stats) => stats,
            Err(err) => {
                $events.error(&err);
                process::exit(1);
            }
        }
    };
}

impl CachegrindRunner {
    /// The workflow is as follows:
    ///
    /// 1. Run the benchmark function once to understand how many iterations are necessary for warm-up, `n`.
    /// 2. Run the *baseline* with `n + 1` iterations terminating after the setup on the last iteration.
    ///    I.e., the "timing" of this run is `(n + 1) * setup + n * bench + const`.
    /// 3. Run the full benchmark with `n + 1` iterations. The "timing" of this run is
    ///    `(n + 1) * setup + (n + 1) * bench + const`.
    /// 4. Subtract baseline stats from the full stats. The difference is equal to `bench`.
    fn run_benchmark(mut self) {
        let final_baseline_path = format!(
            "{}/{}.baseline.cachegrind",
            self.options.cachegrind_out_dir, self.id
        );
        let final_full_path = format!("{}/{}.cachegrind", self.options.cachegrind_out_dir, self.id);
        let old_baseline = self.load_and_backup_summary(&final_baseline_path);
        let prev_stats = old_baseline.and_then(|baseline| {
            let full = self.load_and_backup_summary(&final_full_path)?;
            Some(full - baseline)
        });

        let baseline_path = format!("{final_baseline_path}~");
        let full_path = format!("{final_full_path}~");

        // Use `baseline_path` in case we won't run the baseline after calibration
        let command = self.options.cachegrind_wrapper(&baseline_path);
        self.reporter.start_execution();
        let cachegrind_result = cachegrind::spawn_instrumented(SpawnArgs {
            command,
            out_path: &baseline_path,
            this_executable: &self.this_executable,
            id: &self.id,
            iterations: 2,
            is_baseline: true,
        });
        let summary = unwrap_summary!(self.reporter, cachegrind_result);

        // FIXME: handle `warm_up_instructions == 0` specially
        let estimated_iterations = self.options.warm_up_instructions / summary.total_instructions();
        let estimated_iterations = estimated_iterations.clamp(1, self.options.max_iterations);
        let baseline = if estimated_iterations == 1 {
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
            unwrap_summary!(self.reporter, cachegrind_result)
        };
        self.reporter.baseline_computed(&baseline);

        let command = self.options.cachegrind_wrapper(&full_path);
        let cachegrind_result = cachegrind::spawn_instrumented(SpawnArgs {
            command,
            out_path: &full_path,
            this_executable: &self.this_executable,
            id: &self.id,
            iterations: estimated_iterations + 1,
            is_baseline: false,
        });
        let full = unwrap_summary!(self.reporter, cachegrind_result);
        let stats = full - baseline;

        // (Almost) atomically move cachegrind files to their final locations, so that the following benchmark runs
        // don't output nonsense if the benchmark is interrupted. There's still a risk that the baseline file
        // will get updated and the full output will be not, but it's significantly lower.
        let io_result = fs::rename(&baseline_path, &final_baseline_path);
        unwrap_summary!(self.reporter, io_result);
        let io_result = fs::rename(&full_path, &final_full_path);
        unwrap_summary!(self.reporter, io_result);

        self.reporter.ok(&BenchmarkOutput { stats, prev_stats });
    }

    fn report_benchmark_result(mut self) {
        let baseline_path = format!(
            "{}/{}.baseline.cachegrind",
            self.options.cachegrind_out_dir, self.id
        );
        let full_path = format!("{}/{}.cachegrind", self.options.cachegrind_out_dir, self.id);
        let Some(baseline) = self.load_summary(&baseline_path) else {
            self.reporter.warning(&"no data for benchmark");
            return;
        };
        let Some(full) = self.load_summary(&full_path) else {
            self.reporter.warning(&"no data for benchmark");
            return;
        };
        let stats = full - baseline;

        let old_baseline_path = format!("{baseline_path}.old");
        let old_full_path = format!("{full_path}.old");
        let old_baseline = self.load_summary(&old_baseline_path);
        let prev_stats =
            old_baseline.and_then(|baseline| Some(self.load_summary(&old_full_path)? - baseline));

        self.reporter.ok(&BenchmarkOutput { stats, prev_stats });
    }

    fn load_summary(&mut self, path: &str) -> Option<CachegrindStats> {
        fs::File::open(path)
            .ok()
            .and_then(|file| match CachegrindStats::new(file, path) {
                Ok(summary) => Some(summary),
                Err(err) => {
                    self.reporter.warning(&err);
                    None
                }
            })
    }

    fn load_and_backup_summary(&mut self, path: &str) -> Option<CachegrindStats> {
        let summary = self.load_summary(path);
        if summary.is_some() {
            let backup_path = format!("{path}.old");
            if let Err(err) = fs::copy(path, &backup_path) {
                let err = format!("Failed backing up cachegrind baseline `{path}`: {err}");
                self.reporter.warning(&err);
            }
        }
        summary
    }
}

#[derive(Debug)]
enum BencherInner {
    Main(MainBencher),
    Cachegrind(CachegrindOptions),
}

/// Benchmarking manager providing ability to define and run benchmarks.
///
/// # Examples
///
/// See [crate docs](index.html) for the examples of usage.
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
    /// Adds a reporter to the bencher. Beware that bencher initialization may skew benchmark results.
    #[doc(hidden)] // not stable yet
    pub fn add_reporter(&mut self, reporter: impl Reporter + 'static) -> &mut Self {
        if let BencherInner::Main(bencher) = &mut self.inner {
            bencher.reporter.0.push(Box::new(reporter));
        }
        self
    }

    /// Gets the benchmarking mode.
    pub fn mode(&self) -> BenchMode {
        match &self.inner {
            BencherInner::Main(bencher) => bencher.mode.mode(),
            BencherInner::Cachegrind(_) => BenchMode::Bench,
        }
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
