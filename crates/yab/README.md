# Yet Another Benchmarking framework powered by cachegrind

[![CI](https://github.com/slowli/yab/actions/workflows/ci.yml/badge.svg)](https://github.com/slowli/yab/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%2FApache--2.0-blue)](https://github.com/slowli/yab#license)
![rust 1.75+ required](https://img.shields.io/badge/rust-1.75+-blue.svg?label=Required%20Rust)

**Documentation:**
[![crate docs (main)](https://img.shields.io/badge/main-yellow.svg?label=docs)](https://slowli.github.io/yab/yab/)

YAB is **Y**et **A**nother **B**enchmarking framework powered by [`cachegrind`] from the Valgrind tool suite.
It collects reproducible measurements of Rust code (e.g., the number of executed instructions,
number of L1 and L2/L3 cache hits and RAM accesses), making it possible to use in CI etc.

## Features

- Supports newer `cachegrind` versions and customizing the `cachegrind` wrapper.
- Supports capturing only instruction counts (i.e., not simulating CPU caches).
- Conditionally injects `CACHEGRIND_{START|STOP}_INSTRUMENTATION` macros (available in `cachegrind`
  3.22.0+) allowing for more precise measurements.
- Supports configurable warm-up (defined in terms of executed instructions) before the capture.

## Usage

Define a benchmark binary and include it into your crate manifest:

```toml
[dev-dependencies]
yab = "0.1.0"

[[bench]]
name = "your_bench"
harness = false
```

In the bench source (`benches/your_bench.rs` in the example above), define a function with signature `fn(&mut yab::Bencher)`
and wrap it in the `yab::main!` macro:

```rust
use yab::Bencher;

fn benchmarks(bencher: &mut Bencher) {
    // define your benchmarking code here
}

yab::main!(benchmarks);
```

Run benchmarks as usual using `cargo bench` (or `cargo test --bench ...` to test them).

### Configuration options

Run `cargo bench ... -- --help` to get help on the supported configuration options. Some of the
common options are:

- `--list`: lists benchmarks without running them.
- `--print`: prints results of the latest run instead of running benchmarks.
- `--jobs N` / `-j N`: specifies the number of benchmarks to run in parallel. By default, it's equal
  to the number of logical CPUs in the system.
- `--verbose`, `--quiet`: increases or decreases verbosity of benchmarking output.

### Examples

```rust
use yab::{black_box, Bencher, BenchmarkId};

/// Suppose we want to benchmark this function
fn fibonacci(n: u64) -> u64 {
    match n {
        0 | 1 => 1,
        n => fibonacci(n - 1) + fibonacci(n - 2),
    }
}

fn benchmarks(bencher: &mut Bencher) {
    // Benchmark simple functions.
    bencher
        .bench("fib_short", || fibonacci(black_box(10)))
        .bench("fib_long", || fibonacci(black_box(30)));
    // It's possible to benchmark parametric functions as well:
    for n in [15, 20, 25] {
        bencher.bench(
            BenchmarkId::new("fib", n),
            || fibonacci(black_box(n)),
        );
    }
    // To account for setup and/or teardown, you may use `bench_with_capture`
    bencher.bench_with_capture("fib_capture", |capture| {
        // This will not be included into captured stats.
        black_box(fibonacci(black_box(30)));
        // This will be the only captured segment.
        let output = capture.measure(|| fibonacci(black_box(10)));
        // This assertion won't be captured either
        assert_eq!(output, 55);
    });
}

yab::main!(benchmarks);
```

## Limitations

- `cachegrind` has somewhat limited platform support (e.g., doesn't support Windows).
- `cachegrind` uses simplistic / outdated CPU cache simulation to the point that recent versions
  disable this simulation altogether by default.
- `cachegrind` has limited support when simulating multi-threaded environment.
- Even small changes in the benchmarked code can lead to (generally small) divergences in the measured stats.

## Alternatives and similar tools

- This crate is heavily inspired by [`iai`](https://crates.io/crates/iai), *the* original `cachegrind`-based
  benchmarking framework for Rust.
- [`iai-callgrind`](https://crates.io/crates/iai-callgrind) is an extended / reworked fork of `iai`.
  Compared to it, `yab` prefers simplicity to versatility.
- Benchmarking APIs are inspired by [`criterion`](https://crates.io/crates/criterion).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE)
or [MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in `yab` by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.

[`cachegrind`]: https://valgrind.org/docs/manual/cg-manual.html
