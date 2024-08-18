# Yet Another Benchmarking framework for Rust

YAB is **Y**et **A**nother **B**enchmarking framework powered by [`cachegrind`] from the Valgrind tool suite.
It collects reproducible measurements of Rust code (e.g., the number of executed instructions,
number of L1 and L2/L3 cache hits and RAM accesses), making it possible to use in CI etc.

## Project overview

The project consists of the following crates:

- [`yab`](crates/yab): The benchmarking framework itself
- [`yab-e2e-tests`](e2e-tests): End-to-end tests for the framework.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE)
or [MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in `yab` by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.

[`cachegrind`]: https://valgrind.org/docs/manual/cg-manual.html
