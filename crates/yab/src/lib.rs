pub use std::hint::black_box;

pub use crate::{
    bencher::Bencher,
    cachegrind::{AccessSummary, CachegrindSummary, Instrumentation},
    id::BenchmarkId,
    output::{BenchmarkOutput, BenchmarkProcessor},
};

mod bencher;
mod cachegrind;
mod id;
mod options;
mod output;
mod reporter;
mod utils;
