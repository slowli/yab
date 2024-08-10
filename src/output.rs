use std::fmt;

use crate::{BenchmarkId, CachegrindSummary};

#[derive(Debug)]
#[non_exhaustive]
pub struct BenchmarkOutput {
    pub summary: CachegrindSummary,
    pub old_summary: Option<CachegrindSummary>,
}

pub trait BenchmarkProcessor: 'static + Send + fmt::Debug {
    fn process_benchmark(&mut self, id: &BenchmarkId, output: BenchmarkOutput);
}

/// Default implementation that does nothing.
impl BenchmarkProcessor for () {
    fn process_benchmark(&mut self, _id: &BenchmarkId, _output: BenchmarkOutput) {
        // Do nothing
    }
}
