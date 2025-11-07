use std::{
    fs,
    io::BufWriter,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use crate::{
    bencher::Baseline,
    options::BenchOptions,
    reporter::{BenchmarkOutput, BenchmarkReporter, ControlFlow, Reporter},
    BenchmarkId,
};

#[derive(Debug)]
pub(crate) struct BaselineSaver {
    out_path: PathBuf,
    stats: Arc<Mutex<Baseline>>,
    breakdown: bool,
    control: Arc<dyn ControlFlow>,
}

impl BaselineSaver {
    pub(crate) fn new(out_path: PathBuf, options: &BenchOptions) -> Self {
        Self {
            out_path,
            stats: Arc::default(),
            breakdown: options.breakdown,
            control: Arc::new(()),
        }
    }
}

impl Reporter for BaselineSaver {
    fn set_control(&mut self, control: &Arc<dyn ControlFlow>) {
        self.control = control.clone();
    }

    fn new_benchmark(&mut self, id: &BenchmarkId) -> Box<dyn BenchmarkReporter> {
        Box::new(BenchmarkBaselineReporter {
            id: id.clone(),
            stats: self.stats.clone(),
            breakdown: self.breakdown,
        })
    }

    fn ok(self: Box<Self>) {
        if let Some(parent_dir) = self.out_path.parent() {
            fs::create_dir_all(parent_dir).unwrap_or_else(|err| {
                self.control.error(&format_args!(
                    "failed creating parent dir for baseline file `{}`: {err}",
                    self.out_path.display()
                ));
            });
        }

        let writer = fs::File::create(&self.out_path).unwrap_or_else(|err| {
            self.control.error(&format_args!(
                "failed creating baseline file `{}`: {err}",
                self.out_path.display()
            ));
        });
        let writer = BufWriter::new(writer);

        let stats = Arc::into_inner(self.stats).expect("stats leaked");
        let stats = stats.into_inner().expect("stats are poisoned");
        serde_json::to_writer_pretty(writer, &stats).unwrap_or_else(|err| {
            self.control.error(&format_args!(
                "failed writing baseline file `{}`: {err}",
                self.out_path.display()
            ));
        });
    }
}

#[derive(Debug)]
struct BenchmarkBaselineReporter {
    id: BenchmarkId,
    stats: Arc<Mutex<Baseline>>,
    breakdown: bool,
}

impl BenchmarkReporter for BenchmarkBaselineReporter {
    fn ok(self: Box<Self>, output: &BenchmarkOutput) {
        let mut baseline = self.stats.lock().expect("baseline is poisoned");
        let mut stats = output.stats.clone();
        if self.breakdown {
            // Retain functions above the noise level (0.1% of total instructions).
            let threshold = stats.summary.total_instructions() / 1_000;
            stats
                .breakdown
                .retain(|_, fn_stats| fn_stats.total_instructions() >= threshold);
        } else {
            // Do not include breakdown in the saved baseline
            stats.breakdown.clear();
        }
        baseline.insert(self.id.to_string(), stats);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RegressionChecker {
    threshold: f64,
    regressed_benches: Arc<Mutex<Vec<(BenchmarkId, f64)>>>,
    control: Arc<dyn ControlFlow>,
}

impl RegressionChecker {
    pub fn new(threshold: f64) -> Self {
        Self {
            threshold,
            regressed_benches: Arc::default(),
            control: Arc::new(()),
        }
    }
}

impl Reporter for RegressionChecker {
    fn set_control(&mut self, control: &Arc<dyn ControlFlow>) {
        self.control = control.clone();
    }

    fn new_benchmark(&mut self, id: &BenchmarkId) -> Box<dyn BenchmarkReporter> {
        Box::new(RegressionBenchmarkChecker {
            parent: self.clone(),
            id: id.clone(),
        })
    }

    fn ok(self: Box<Self>) {
        use std::fmt::Write as _;

        let regressed_benches = Arc::into_inner(self.regressed_benches)
            .expect("`regressed_benches` leaked")
            .into_inner()
            .expect("`regressed_benches` is poisoned");

        if !regressed_benches.is_empty() {
            let len = regressed_benches.len();
            let mut list = String::new();
            for (i, (id, regression)) in regressed_benches.iter().enumerate() {
                write!(&mut list, "  {id}: {:+.1}%", regression * 100.0).unwrap();
                if i + 1 < len {
                    writeln!(&mut list).unwrap();
                }
            }

            self.control.error(&format_args!(
                "{len} bench{plural} ha{s_or_ve} regressed by >{threshold:.1}%:\n{list}",
                plural = if len == 1 { "" } else { "s" },
                s_or_ve = if len == 1 { "s" } else { "ve" },
                threshold = self.threshold * 100.0
            ));
        }
    }
}

#[derive(Debug)]
struct RegressionBenchmarkChecker {
    parent: RegressionChecker,
    id: BenchmarkId,
}

impl BenchmarkReporter for RegressionBenchmarkChecker {
    fn ok(self: Box<Self>, output: &BenchmarkOutput) {
        let Some(prev_stats) = &output.prev_stats else {
            return;
        };
        let current = output.stats.summary.total_instructions();
        let prev = prev_stats.summary.total_instructions();
        let Some(regression) = current.checked_sub(prev) else {
            return; // no regression happened
        };

        #[allow(clippy::cast_precision_loss)] // OK for comparisons
        let regression = regression as f64 / prev as f64;
        if regression > self.parent.threshold {
            self.parent
                .control
                .for_benchmark(&self.id)
                .warning(&format_args!(
                    "bench has regressed by {:.1}%",
                    regression * 100.0
                ));
            self.parent
                .regressed_benches
                .lock()
                .expect("`regressed_benches` is poisoned")
                .push((self.id, regression));
        }
    }
}
