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

pub fn black_box<T>(dummy: T) -> T {
    unsafe {
        let ret = std::ptr::read_volatile(&dummy);
        std::mem::forget(dummy);
        ret
    }
}
