use std::fmt;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::{BenchmarkId, CachegrindSummary};

#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub struct BenchmarkOutput {
    pub summary: CachegrindSummary,
    pub old_summary: Option<CachegrindSummary>,
}

pub trait BenchmarkProcessor: 'static + Send + Sync + fmt::Debug {
    fn process_benchmark(&self, id: &BenchmarkId, output: BenchmarkOutput);
}

/// Default processor that does nothing.
impl BenchmarkProcessor for () {
    fn process_benchmark(&self, _id: &BenchmarkId, _output: BenchmarkOutput) {
        // Do nothing
    }
}
