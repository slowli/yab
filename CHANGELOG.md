# Changelog

All notable changes to this project will be documented in this file.
The project adheres to [Semantic Versioning](http://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Support saving / loading named baselines similarly to `criterion`.
- Support multiple captures in a single benchmark.

### Changed

- Use regular expressions to match benchmark IDs.
- Bump minimum supported Rust version to 1.85.
- Rework reporter traits by extracting logging functionality to a separate trait.

### Fixed

- Better handle benchmark interrupts by saving cachegrind stats to temporary files.

## 0.1.0 - 2024-10-28

The initial release of `yab`.
