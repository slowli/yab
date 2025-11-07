//! CLI snapshot tests.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, MutexGuard, Once, PoisonError},
    time::Duration,
};

use regex::Regex;
use tempfile::TempDir;
use term_transcript::{
    svg::{Template, TemplateOptions},
    test::{MatchKind, TestConfig},
    ShellOptions, StdShell, UserInput,
};

const MOCK_CACHEGRIND_PATH: &str = env!("CARGO_BIN_EXE_mock-cachegrind");

#[derive(Debug)]
struct TestLock {
    dir: TempDir,
    // We want tests to be sequential; otherwise, there's a high probability that they fail when locking
    // package cache / build dir.
    _mutex: MutexGuard<'static, ()>,
}

impl TestLock {
    fn new() -> Self {
        static PREPARE_LOCK: Once = Once::new();
        static TEST_LOCK: Mutex<()> = Mutex::new(());

        PREPARE_LOCK.call_once(|| {
            Command::new("cargo")
                .args(["bench", "--bench", "all", "--no-run"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("failed compiling benchmarks");
        });

        Self {
            dir: TempDir::new().unwrap(),
            _mutex: TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner),
        }
    }
}

fn lib_snapshot(name: &str) -> PathBuf {
    let mut snapshot_path = Path::new("../crates/yab/examples").join(name);
    snapshot_path.set_extension("svg");
    snapshot_path
}

fn transform_output<'a>(output_lines: impl Iterator<Item = &'a str>) -> String {
    // Normalize unpredictable parts of the benching output: durations, bench options, cachegrind version, and file locations.
    let duration_regex = Regex::new(r"\(\d+(\.\d+)?[num]?s\)").unwrap();
    let options_regex = Regex::new(r"BenchOptions \{.*}").unwrap();
    let cachegrind_version_regex = Regex::new(r"cachegrind with version (.*)$").unwrap();
    let code_location_regex = Regex::new(r"(?<file>e2e-tests/src/lib\.rs):(?<line>\d+)").unwrap();

    let mut should_output_line = false;
    let mut buffer = String::new();
    for line in output_lines {
        let current_output_line = should_output_line;
        if line.contains("Running") && line.contains("benches/all.rs") {
            should_output_line = true;
        }
        if line.contains("bench failed, to rerun pass") {
            // Truncate the following "Caused by" diagnostic output containing unpredictable paths etc.
            should_output_line = false;
        }
        if !current_output_line {
            continue;
        }

        // Replace variable segments
        let replaced =
            cachegrind_version_regex.replace(line, "cachegrind with version valgrind-3.23.0");
        let replaced = options_regex.replace(&replaced, "BenchOptions { .. }");
        let replaced = duration_regex.replace(&replaced, "(10ms)");
        let replaced = code_location_regex.replace(&replaced, "$file:50");
        buffer += &replaced;
        buffer.push('\n');
    }
    buffer
}

fn test_config(sequential: bool) -> (TestConfig<StdShell>, TestLock) {
    let lock = TestLock::new();
    let target_path = lock.dir.path().join("target");

    let mut shell_options = ShellOptions::sh()
        .with_env("COLOR", "always")
        .with_env("CACHEGRIND_WRAPPER", MOCK_CACHEGRIND_PATH)
        .with_env("CACHEGRIND_OUT_DIR", &target_path)
        .with_io_timeout(Duration::from_secs(1));
    if sequential {
        shell_options = shell_options.with_env("CACHEGRIND_JOBS", "1");
    }

    let config: TestConfig<_> = TestConfig::new(shell_options).with_transform(|transcript| {
        for interaction in transcript.interactions_mut() {
            let output_lines = interaction.output().as_ref().lines();
            let transformed_output = transform_output(output_lines);
            interaction.set_output(transformed_output.into());
        }
    });
    (config.with_match_kind(MatchKind::Precise), lock)
}

fn plain_template() -> Template {
    let template_options = TemplateOptions {
        window_frame: true,
        ..TemplateOptions::default()
    };
    Template::new(template_options)
}

#[test]
fn basic_transcript() {
    let (config, _lock) = test_config(true);
    config
        .with_template(plain_template())
        .test(lib_snapshot("basic"), ["cargo bench --bench all -- fib/"]);
}

#[test]
fn quiet_transcript() {
    let (config, _lock) = test_config(true);
    config.with_template(plain_template()).test(
        lib_snapshot("quiet"),
        ["cargo bench --bench all -- --quiet fib"],
    );
}

#[test]
fn comparison_transcript() {
    let (config, _lock) = test_config(false);
    config.with_template(plain_template()).test(
        lib_snapshot("comparison"),
        [
            UserInput::command("cargo bench --bench all fib_short"),
            UserInput::command("export CACHEGRIND_WRAPPER=\"$CACHEGRIND_WRAPPER:--profile=cmp\"")
                .hide(),
            UserInput::command("cargo bench --bench all fib_short\n# after some changes..."),
        ],
    );
}

#[test]
fn comparing_to_baseline() {
    let (config, _lock) = test_config(false);
    config.with_template(plain_template()).test(
        lib_snapshot("cmp-baseline"),
        [UserInput::command(
            "cargo bench --bench all -- --vs pub:cmp fib_short\n\
                # Compare current `fib_short` impl to the public `cmp` baseline\n\
                # (one in the `benches/all` dir)",
        )],
    );
}

#[test]
fn baseline_regression_failure() {
    let (config, _lock) = test_config(true);
    config.with_template(plain_template()).test(
        lib_snapshot("baseline-regression"),
        [
            UserInput::command("export CACHEGRIND_REGRESSION_THRESHOLD=0.01"),
            UserInput::command("cargo bench --bench all -- --vs pub:cmp -q random_walk"),
        ],
    );
}

#[test]
fn verbose_transcript() {
    let (config, _lock) = test_config(false);
    config.with_template(plain_template()).test(
        lib_snapshot("verbose"),
        [
            UserInput::command("cargo bench --bench all -- --quiet random_walk/10000000"),
            UserInput::command("export CACHEGRIND_WRAPPER=\"$CACHEGRIND_WRAPPER:--profile=cmp\"")
                .hide(),
            UserInput::command("cargo bench --bench all -- --verbose random_walk/10000000\n# after some changes..."),
        ],
    );
}

#[test]
fn breakdown() {
    let (config, _lock) = test_config(false);
    let template_options = TemplateOptions {
        window_frame: true,
        width: 850,
        wrap: None,
        ..TemplateOptions::default()
    };
    config.with_template(Template::new(template_options)).test(
        lib_snapshot("breakdown"),
        [UserInput::command(
            "cargo bench --bench all -- --quiet --breakdown collect/hash_set",
        )],
    );
}

#[test]
fn printing_baseline() {
    let (config, _lock) = test_config(true);
    config.with_template(plain_template()).test(
        lib_snapshot("print-baseline"),
        [UserInput::command(
            "cargo bench --bench all -- --print pub:cmp --quiet",
        )],
    );
}
