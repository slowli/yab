use std::fmt;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::{BenchmarkId, CachegrindStats};

/// Output produced by the [`Bencher`](crate::Bencher) for a single benchmark.
#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub struct BenchmarkOutput {
    /// Latest / current stats for the benchmark.
    pub stats: CachegrindStats,
    /// Previous stats for the benchmark.
    pub prev_stats: Option<CachegrindStats>,
}

/// Handler for benchmarking output that allows to extend or modify benchmarking logic.
pub trait BenchmarkProcessor: 'static + Send + Sync + fmt::Debug {
    /// Handles output for a single benchmark. This method can be called concurrently from multiple threads.
    fn process_benchmark(&self, id: &BenchmarkId, output: BenchmarkOutput);
}

/// Default processor that does nothing.
impl BenchmarkProcessor for () {
    fn process_benchmark(&self, _id: &BenchmarkId, _output: BenchmarkOutput) {
        // Do nothing
    }
}
