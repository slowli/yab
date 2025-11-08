//! [`Bencher`] and tightly related types.

use std::{
    collections::HashMap,
    env, fmt, fs,
    io::BufReader,
    iter, mem, panic,
    path::Path,
    sync::{Arc, OnceLock},
    thread,
    thread::JoinHandle,
};

use crate::{
    cachegrind,
    cachegrind::{CachegrindOutput, SpawnArgs},
    options::{BenchOptions, CachegrindOptions, IdMatcher, Options},
    reporter::{
        baseline::{BaselineSaver, RegressionChecker},
        BenchmarkOutput, BenchmarkReporter, Logger, PrintingReporter, Reporter, SeqReporter,
    },
    utils::Semaphore,
    BenchmarkId, Capture,
};

pub(crate) type Baseline = HashMap<String, CachegrindOutput>;

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
    PrintResults {
        current: Option<Baseline>,
    },
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
            BenchMode::PrintResults => Self::PrintResults { current: None },
        }
    }

    fn mode(&self) -> BenchMode {
        match self {
            Self::Test { .. } => BenchMode::Test,
            Self::Bench { .. } => BenchMode::Bench,
            Self::List => BenchMode::List,
            Self::PrintResults { .. } => BenchMode::PrintResults,
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
    baseline: Arc<OnceLock<Baseline>>,
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
                            .logger
                            .fatal(&"At least one of benchmarking jobs failed");
                    }
                }
            }
            BenchModeData::Test { should_fail } if *should_fail => {
                self.reporter.logger.fatal(&"There were test failures");
            }
            _ => { /* no special handling required */ }
        }
        self.reporter.ok_all();
    }
}

impl MainBencher {
    fn new(options: BenchOptions) -> Self {
        let mut printer =
            PrintingReporter::new(options.styling(), options.verbosity(), options.breakdown);
        let logger = Arc::new(printer.to_logger());

        options.report(&mut printer);
        let mode = BenchModeData::new(&options);
        if matches!(mode, BenchModeData::Bench { .. }) {
            match cachegrind::check() {
                Ok(version) => {
                    printer.report_debug(format_args!("Using cachegrind with version {version}"));
                }
                Err(err) => {
                    logger.fatal(&err);
                }
            }
        }

        let id_matcher = match options.id_matcher() {
            Ok(matcher) => matcher,
            Err(err) => {
                logger.fatal(&err);
            }
        };

        let mut reporter = SeqReporter::new(logger);
        reporter.push(Box::new(printer));
        if let Some(path) = options.save_baseline_path() {
            let saver = BaselineSaver::new(path, &options);
            reporter.push(Box::new(saver));
        }
        if let Some(threshold) = options.regression_threshold() {
            reporter.push(Box::new(RegressionChecker::new(threshold)));
        }

        Self {
            options,
            id_matcher,
            mode,
            reporter,
            baseline: Arc::default(),
        }
    }

    fn bench(
        &mut self,
        id: &BenchmarkId,
        capture_names: &[&'static str],
        mut bench_fn: impl FnMut(Vec<Capture>),
    ) {
        if !self.id_matcher.matches(id) {
            return;
        }

        match &mut self.mode {
            BenchModeData::Test { should_fail } => {
                let test_reporter = self.reporter.new_test(id);
                let captures: Vec<_> = iter::repeat_with(Capture::no_op)
                    .take(capture_names.len())
                    .collect();
                // Run the function once w/o instrumentation.
                if cfg!(panic = "unwind") {
                    let wrapped = panic::AssertUnwindSafe(move || bench_fn(captures));
                    if let Err(err) = panic::catch_unwind(wrapped) {
                        test_reporter.fail(&err);
                        *should_fail = true;
                        return;
                    }
                } else {
                    bench_fn(captures);
                }
                test_reporter.ok();
            }
            BenchModeData::Bench {
                jobs_semaphore,
                jobs,
                this_executable,
            } => {
                let executors =
                    capture_names
                        .iter()
                        .enumerate()
                        .map(|(active_capture, &capture_name)| {
                            let mut id = id.clone();
                            if !capture_name.is_empty() {
                                id.capture = Some(capture_name);
                            }
                            CachegrindRunner {
                                options: self.options.clone(),
                                this_executable: this_executable.to_owned(),
                                reporter: self.reporter.new_benchmark(&id),
                                logger: self.reporter.logger.clone().for_benchmark(&id),
                                id,
                                active_capture,
                                baseline: self.baseline.clone(),
                            }
                        });

                if jobs_semaphore.capacity() == 1 {
                    // Run the executors synchronously in order to have deterministic ordering
                    for executor in executors {
                        executor.run_benchmark();
                    }
                } else {
                    jobs.extend(executors.map(|executor| {
                        let jobs_semaphore = jobs_semaphore.clone();
                        thread::spawn(move || {
                            let _permit = jobs_semaphore.acquire_owned();
                            executor.run_benchmark();
                        })
                    }));
                }
            }
            BenchModeData::List => {
                PrintingReporter::report_list_item(id);
            }
            BenchModeData::PrintResults { current } => {
                for (active_capture, &capture_name) in capture_names.iter().enumerate() {
                    let mut id = id.clone();
                    if !capture_name.is_empty() {
                        id.capture = Some(capture_name);
                    }
                    let executor = CachegrindRunner {
                        options: self.options.clone(),
                        reporter: self.reporter.new_benchmark(&id),
                        logger: self.reporter.logger.clone().for_benchmark(&id),
                        // `this_executable` isn't used, so it's fine to set it to an empty string
                        this_executable: String::new(),
                        id,
                        active_capture,
                        baseline: self.baseline.clone(),
                    };
                    executor.report_benchmark_result(current);
                }
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
    logger: Arc<dyn Logger>,
    id: BenchmarkId,
    active_capture: usize,
    baseline: Arc<OnceLock<Baseline>>,
}

impl dyn Logger {
    fn unwrap_result<T, E: fmt::Display>(&self, result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => {
                self.fatal(&err);
            }
        }
    }
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
        let out_dir = &self.options.cachegrind_out_dir;
        let baseline_path = out_dir.join(format!("{}.baseline.cachegrind~", self.id));
        let full_path = out_dir.join(format!("{}.cachegrind~", self.id));
        let final_baseline_path = out_dir.join(format!("{}.baseline.cachegrind", self.id));
        let final_full_path = out_dir.join(format!("{}.cachegrind", self.id));

        let prev_stats = if let Some(path) = self.options.baseline_path() {
            let id = self.id.to_string();
            self.ensure_baseline(&path).get(&id).cloned()
        } else {
            let old_baseline = self.load_and_backup_output(&final_baseline_path);
            old_baseline.and_then(|baseline| {
                let full = self.load_and_backup_output(&final_full_path)?;
                Some(full - baseline)
            })
        };

        // Use `baseline_path` in case we won't run the baseline after calibration
        let command = self.options.cachegrind_wrapper(&baseline_path);
        self.reporter.start_execution();
        let cachegrind_result = cachegrind::spawn_instrumented(SpawnArgs {
            command,
            out_path: &baseline_path,
            this_executable: &self.this_executable,
            id: &self.id,
            active_capture: self.active_capture,
            iterations: 2,
            is_baseline: true,
        });
        let output = self.logger.unwrap_result(cachegrind_result);

        // FIXME: handle `warm_up_instructions == 0` specially
        let estimated_iterations =
            self.options.warm_up_instructions / output.summary.total_instructions();
        let estimated_iterations = estimated_iterations.clamp(1, self.options.max_iterations);
        let baseline = if estimated_iterations == 1 {
            output
        } else {
            // This will override calibration output, which is exactly what we need.
            let command = self.options.cachegrind_wrapper(&baseline_path);
            let cachegrind_result = cachegrind::spawn_instrumented(SpawnArgs {
                command,
                out_path: &baseline_path,
                this_executable: &self.this_executable,
                id: &self.id,
                active_capture: self.active_capture,
                iterations: estimated_iterations + 1,
                is_baseline: true,
            });
            self.logger.unwrap_result(cachegrind_result)
        };
        self.reporter.baseline_computed(&baseline.summary);

        let command = self.options.cachegrind_wrapper(&full_path);
        let cachegrind_result = cachegrind::spawn_instrumented(SpawnArgs {
            command,
            out_path: &full_path,
            this_executable: &self.this_executable,
            id: &self.id,
            active_capture: self.active_capture,
            iterations: estimated_iterations + 1,
            is_baseline: false,
        });
        let full = self.logger.unwrap_result(cachegrind_result);
        let stats = full - baseline;

        // (Almost) atomically move cachegrind files to their final locations, so that the following benchmark runs
        // don't output nonsense if the benchmark is interrupted. There's still a risk that the baseline file
        // will get updated and the full output will be not, but it's significantly lower.
        let io_result = fs::rename(&baseline_path, &final_baseline_path);
        self.logger.unwrap_result(io_result);
        let io_result = fs::rename(&full_path, &final_full_path);
        self.logger.unwrap_result(io_result);

        self.reporter.ok(&BenchmarkOutput { stats, prev_stats });
    }

    fn report_benchmark_result(mut self, printed_baseline: &mut Option<Baseline>) {
        let out_dir = &self.options.cachegrind_out_dir;
        let baseline_path = out_dir.join(format!("{}.baseline.cachegrind", self.id));
        let full_path = out_dir.join(format!("{}.cachegrind", self.id));
        let old_baseline_path = out_dir.join(format!("{}.baseline.cachegrind.old", self.id));
        let old_full_path = out_dir.join(format!("{}.cachegrind.old", self.id));

        let stats = if let Some(path) = self.options.print_baseline_path() {
            let baseline = printed_baseline
                .get_or_insert_with(|| Self::load_baseline(self.logger.as_ref(), &path));
            if let Some(stats) = baseline.get(&self.id.to_string()) {
                stats.clone()
            } else {
                self.logger.warning(&"no data for benchmark");
                return;
            }
        } else {
            let Some(baseline) = self.load_output(&baseline_path) else {
                self.logger.warning(&"no data for benchmark");
                return;
            };
            let Some(full) = self.load_output(&full_path) else {
                self.logger.warning(&"no data for benchmark");
                return;
            };
            full - baseline
        };

        let prev_stats = if let Some(path) = self.options.baseline_path() {
            let id = self.id.to_string();
            self.ensure_baseline(&path).get(&id).cloned()
        } else if self.options.has_print_baseline() {
            // Do not load default / unnamed prev stats if the current baseline is specified.
            None
        } else {
            let old_baseline = self.load_output(&old_baseline_path);
            old_baseline.and_then(|baseline| Some(self.load_output(&old_full_path)? - baseline))
        };

        self.reporter.ok(&BenchmarkOutput { stats, prev_stats });
    }

    fn load_output(&mut self, path: &Path) -> Option<CachegrindOutput> {
        fs::File::open(path)
            .ok()
            .and_then(|file| match CachegrindOutput::new(file, path) {
                Ok(summary) => Some(summary),
                Err(err) => {
                    self.logger.warning(&err);
                    None
                }
            })
    }

    fn ensure_baseline(&self, path: &Path) -> &Baseline {
        self.baseline
            .get_or_init(|| Self::load_baseline(self.logger.as_ref(), path))
    }

    fn load_baseline(logger: &dyn Logger, path: &Path) -> Baseline {
        match Self::load_baseline_inner(path) {
            Ok(baseline) => baseline,
            Err(err) => {
                logger.fatal(&format_args!(
                    "failed reading baseline from {}: {err}",
                    path.display()
                ));
            }
        }
    }

    fn load_baseline_inner(path: &Path) -> std::io::Result<Baseline> {
        let reader = fs::File::open(path)?;
        serde_json::from_reader(BufReader::new(reader)).map_err(Into::into)
    }

    fn load_and_backup_output(&mut self, path: &Path) -> Option<CachegrindOutput> {
        let summary = self.load_output(path);
        if summary.is_some() {
            let mut backup_path = path.to_owned();
            // `unwrap()`s are safe because we control filenames
            let current_extension = backup_path.extension().unwrap().to_str().unwrap();
            backup_path.set_extension(format!("{current_extension}.old"));

            if let Err(err) = fs::copy(path, &backup_path) {
                let err = format!(
                    "Failed backing up cachegrind baseline `{path}`: {err}",
                    path = path.display()
                );
                self.logger.warning(&err);
            }
        }
        summary
    }
}

#[derive(Debug)]
enum BencherInner {
    Main(Box<MainBencher>),
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

impl Bencher {
    #[doc(hidden)] // should only be used from `yab::main!()` macro
    pub fn new(bench_name: &'static str) -> Self {
        let inner = match Options::new() {
            Options::Bench(mut options) => {
                options.bench_name = bench_name;
                BencherInner::Main(Box::new(MainBencher::new(options)))
            }
            Options::Cachegrind(options) => BencherInner::Cachegrind(options),
        };
        Self { inner }
    }

    /// Adds a reporter to the bencher. Beware that bencher initialization may skew benchmark results.
    #[doc(hidden)] // not stable yet
    pub fn add_reporter(&mut self, reporter: impl Reporter + 'static) -> &mut Self {
        if let BencherInner::Main(bencher) = &mut self.inner {
            bencher.reporter.push(Box::new(reporter));
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
    #[inline]
    pub fn bench<T>(
        &mut self,
        id: impl Into<BenchmarkId>,
        mut bench_fn: impl FnMut() -> T,
    ) -> &mut Self {
        self.bench_inner(&id.into(), &[""], move |[capture]| {
            capture.measure(&mut bench_fn); // dropping the output is not included into capture
        });
        self
    }

    /// Benchmarks a function with configurable capture interval. This allows set up before starting the capture
    /// and/or post-processing (e.g., assertions) after the capture.
    #[track_caller]
    #[inline]
    pub fn bench_with_capture(
        &mut self,
        id: impl Into<BenchmarkId>,
        mut bench_fn: impl FnMut(Capture),
    ) -> &mut Self {
        self.bench_inner(&id.into(), &[""], move |[capture]| {
            bench_fn(capture);
        });
        self
    }

    /// FIXME
    #[track_caller]
    #[inline]
    pub fn bench_with_captures<const N: usize>(
        &mut self,
        id: impl Into<BenchmarkId>,
        (capture_names, bench_fn): ([&'static str; N], impl FnMut([Capture; N])),
    ) -> &mut Self {
        self.bench_inner(&id.into(), &capture_names, bench_fn);
        self
    }

    fn bench_inner<const N: usize>(
        &mut self,
        id: &BenchmarkId,
        capture_names: &[&'static str],
        mut bench_fn: impl FnMut([Capture; N]),
    ) {
        match &mut self.inner {
            BencherInner::Main(bencher) => {
                bencher.bench(id, capture_names, move |captures| {
                    let captures: [Capture; N] = captures.try_into().unwrap();
                    bench_fn(captures);
                });
            }
            BencherInner::Cachegrind(options) => {
                if *id != options.id.as_str() {
                    return;
                }
                cachegrind::run_instrumented(
                    bench_fn,
                    options.iterations,
                    options.is_baseline,
                    options.active_capture,
                );
            }
        }
    }
}
