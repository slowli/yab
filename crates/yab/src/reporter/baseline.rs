use std::{
    fs,
    io::BufWriter,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use crate::{
    bencher::Baseline,
    options::BenchOptions,
    reporter::{BenchmarkOutput, BenchmarkReporter, Reporter},
    BenchmarkId,
};

#[derive(Debug)]
pub(crate) struct BaselineSaver {
    out_path: PathBuf,
    stats: Arc<Mutex<Baseline>>,
    breakdown: bool,
}

impl BaselineSaver {
    pub(crate) fn new(out_path: PathBuf, options: &BenchOptions) -> Self {
        Self {
            out_path,
            stats: Arc::default(),
            breakdown: options.breakdown,
        }
    }
}

impl Reporter for BaselineSaver {
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
                panic!(
                    "failed creating parent dir for baseline file `{}`: {err}",
                    self.out_path.display()
                );
            });
        }

        let writer = fs::File::create(&self.out_path).unwrap_or_else(|err| {
            panic!(
                "failed creating baseline file `{}`: {err}",
                self.out_path.display()
            )
        });
        let writer = BufWriter::new(writer);

        let stats = Arc::into_inner(self.stats).expect("stats leaked");
        let stats = stats.into_inner().expect("stats are poisoned");
        serde_json::to_writer_pretty(writer, &stats).unwrap_or_else(|err| {
            panic!(
                "failed writing baseline file `{}`: {err}",
                self.out_path.display()
            )
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
