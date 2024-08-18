//! YAB is **Y**et **A**nother **B**enchmarking framework powered by [`cachegrind`] from the Valgrind tool suite.
//! It collects reproducible measurements of Rust code (e.g., the number of executed instructions,
//! number of L1 and L2/L3 cache hits and RAM accesses), making it possible to use in CI etc.
//!
//! # Limitations
//!
//! - `cachegrind` has somewhat limited platform support (e.g., doesn't support Windows).
//! - `cachegrind` uses simplistic / outdated CPU cache simulation to the point that recent versions
//!   disable this simulation altogether by default.
//! - Even small changes in the benchmarked code can lead to (generally small) divergences in the measured stats.
//!
//! # Alternatives and similar tools
//!
//! - This crate is heavily inspired by [`iai`](https://crates.io/crates/iai), *the* original `cachegrind`-based
//!   benchmarking framework for Rust.
//! - [`iai-callgrind`](https://crates.io/crates/iai-callgrind) is an extended / reworked fork of `iai`.
//!   Compared to it, `yab` prefers simplicity to versatility.
//! - Benchmarking APIs are inspired by [`criterion`](https://crates.io/crates/criterion).
//!
//! # Examples
//!
//! The entrypoint for defining benchmarks is [`Bencher`].
//!
//! ```
//! use yab::{black_box, Bencher, BenchmarkId};
//!
//! /// Suppose we want to benchmark this function
//! fn fibonacci(n: u64) -> u64 {
//!     match n {
//!         0 | 1 => 1,
//!         n => fibonacci(n - 1) + fibonacci(n - 2),
//!     }
//! }
//!
//! // Read benchmark configuration from environment / command-line args.
//! let mut bencher = Bencher::default();
//! // Benchmark simple functions.
//! bencher
//!     .bench("fib_short", || fibonacci(black_box(10)))
//!     .bench("fib_long", || fibonacci(black_box(30)));
//! // It's possible to benchmark parametric functions as well:
//! for n in [15, 20, 25] {
//!     bencher.bench(
//!         BenchmarkId::new("fib", n),
//!         || fibonacci(black_box(n)),
//!     );
//! }
//! // To account for setup and/or teardown, you may use `bench_with_capture`
//! bencher.bench_with_capture("fib_capture", |capture| {
//!     // This will not be included into captured stats.
//!     black_box(fibonacci(black_box(30)));
//!     // This will be the only captured segment.
//!     let output = capture.measure(|| fibonacci(black_box(10)));
//!     // This assertion won't be captured either
//!     assert_eq!(output, 55);
//! });
//! ```
//!
//! [`cachegrind`]: https://valgrind.org/docs/manual/cg-manual.html

// Linter settings.
#![warn(missing_debug_implementations, missing_docs, bare_trait_objects)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::must_use_candidate, clippy::module_name_repetitions)]

pub use std::hint::black_box;

pub use crate::{
    bencher::Bencher,
    cachegrind::{AccessSummary, CachegrindStats, Capture, CaptureGuard},
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

/// Wraps a provided function to create the entrypoint for a benchmark executable. The function
/// must have `fn(&mut` [`Bencher`]`)` signature.
///
/// # Examples
///
/// See [crate docs](index.html) for the examples of usage.
#[macro_export]
macro_rules! main {
    ($function:path) => {
        fn main() {
            $function(&mut $crate::Bencher::default());
        }
    };
}
