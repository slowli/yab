//! Benchmark reporting.

use std::{any::Any, fmt};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

pub(crate) use self::{
    printer::{PrintingReporter, Verbosity},
    seq::SeqReporter,
};
use crate::{cachegrind::CachegrindOutput, BenchmarkId, CachegrindStats};

#[cfg(feature = "baselines")]
pub(crate) mod baseline;
mod printer;
mod seq;

/// Output produced by the [`Bencher`](crate::Bencher) for a single benchmark.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub struct BenchmarkOutput {
    /// Latest / current stats for the benchmark.
    pub stats: CachegrindOutput,
    /// Previous stats for the benchmark.
    pub prev_stats: Option<CachegrindOutput>,
}

/// Reporter for benchmarking output that allows to extend or modify benchmarking logic.
#[allow(unused_variables)]
pub trait Reporter: fmt::Debug {
    /// Reports a (non-recoverable) error not related to a particular benchmark.
    /// This is mutually exclusive with [`Self::ok()`].
    ///
    /// The default implementation does nothing.
    fn error(&mut self, error: &dyn fmt::Display) {
        // do nothing
    }

    /// Initializes a test with the specified ID.
    fn new_test(&mut self, id: &BenchmarkId) -> Box<dyn TestReporter> {
        Box::new(())
    }

    /// Initializes a benchmark with the specified ID. Note that the benchmark isn't necessarily
    /// immediately started; the start will be signaled separately via [`BenchmarkReporter::start_execution()`].
    fn new_benchmark(&mut self, id: &BenchmarkId) -> Box<dyn BenchmarkReporter>;

    /// Signals to the reporter that processing tests / benchmarks has successfully completed.
    /// This is mutually exclusive with [`Self::error()`].
    ///
    /// The default implementation does nothing.
    fn ok(self: Box<Self>) {
        // do nothing
    }
}

/// Reporter of events for a single benchmark run in the test mode.
pub trait TestReporter {
    /// Finishes the test successfully.
    fn ok(self: Box<Self>);
    /// Fails the test with the specified panic data.
    fn fail(self: Box<Self>, panic_data: &dyn Any);
}

/// No-op implementation.
impl TestReporter for () {
    fn ok(self: Box<Self>) {
        // do nothing
    }

    fn fail(self: Box<Self>, _panic_data: &dyn Any) {
        // do nothing
    }
}

/// Reporter of events for a single benchmark.
#[allow(unused_variables)]
pub trait BenchmarkReporter: Send + fmt::Debug {
    /// Reports that the benchmark started executing.
    ///
    /// The default implementation does nothing.
    fn start_execution(&mut self) {
        // do nothing
    }

    /// Reports a baseline being computed for a benchmark.
    ///
    /// The default implementation does nothing.
    #[doc(hidden)] // seems too low-level / specific for now
    fn baseline_computed(&mut self, stats: &CachegrindStats) {
        // do nothing
    }

    /// Reports output for a single benchmark.
    fn ok(self: Box<Self>, output: &BenchmarkOutput);

    /// Reports a warning related to the benchmark.
    ///
    /// The default implementation does nothing.
    fn warning(&mut self, warning: &dyn fmt::Display) {
        // do nothing
    }

    /// Reports a (non-recoverable) benchmark error.
    ///
    /// The default implementation does nothing.
    fn error(self: Box<Self>, error: &dyn fmt::Display) {
        // do nothing
    }
}
