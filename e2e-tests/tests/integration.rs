//! Should preferably run in the release mode to emulate real benchmark env, and because some benches
//! are quite slow otherwise.

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::Path,
    process::{Command, Stdio},
};

use yab::{AccessSummary, BenchmarkOutput, FullCachegrindStats};
use yab_e2e_tests::EXPORTER_OUTPUT_VAR;

const EXE_PATH: &str = env!("CARGO_BIN_EXE_yab-e2e-tests");

const EXPECTED_BENCH_NAMES: &[&str] = &[
    "fib_short",
    "fib_long",
    "fib/15",
    "fib/20",
    "fib/25",
    "fib_guard",
    "fib_setup",
    "random_walk/1000000",
    "random_walk/10000000",
];

fn read_outputs(path: &Path) -> HashMap<String, BenchmarkOutput> {
    let reader = fs::File::open(path).unwrap();
    serde_json::from_reader(io::BufReader::new(reader)).unwrap()
}

fn assert_close(new: &FullCachegrindStats, old: &FullCachegrindStats) {
    const THRESHOLD: u64 = 100;

    let points = [
        (new.instructions, old.instructions),
        (new.data_reads, old.data_reads),
        (new.data_writes, old.data_writes),
    ];
    for (new_point, old_point) in points {
        let diff = new_point.total.abs_diff(old_point.total);
        assert!(diff < THRESHOLD, "new={new:?}, old={old:?}");
    }
}

#[test]
fn testing_benchmarks() {
    // Without `--bench` argument, benches should be tested.
    let output = Command::new(EXE_PATH).output().unwrap();
    assert!(output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains('\u{1b}')); // no ANSI escape sequences since stderr is not a TTY

    let test_names: HashSet<_> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("Testing ")?.split_whitespace().next())
        .collect();
    for &name in EXPECTED_BENCH_NAMES {
        assert!(
            test_names.contains(name),
            "{test_names:?} doesn't contain {name}"
        );
    }
}

#[test]
fn testing_with_filter() {
    let output = Command::new(EXE_PATH).arg("fib/").output().unwrap();
    assert!(output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    let test_names: HashSet<_> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("Testing ")?.split_whitespace().next())
        .collect();
    assert_eq!(test_names, HashSet::from(["fib/15", "fib/20", "fib/25"]));
}

#[test]
fn benchmarking_everything() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let out_path = temp_dir.path().join("out.json");
    let target_path = temp_dir.path().join("target");

    let output = Command::new(EXE_PATH)
        .arg("--bench")
        .env(EXPORTER_OUTPUT_VAR, &out_path)
        .env("CACHEGRIND_OUT_DIR", &target_path)
        .output()
        .expect("failed running benches");
    assert!(output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains('\u{1b}')); // no ANSI escape sequences since stderr is not a TTY

    let benchmark_names: HashSet<_> = stderr
        .lines()
        .filter_map(|line| {
            line.strip_prefix("Benchmarking ")?
                .split_whitespace()
                .next()
        })
        .collect();
    for &name in EXPECTED_BENCH_NAMES {
        assert!(
            benchmark_names.contains(name),
            "{benchmark_names:?} doesn't contain {name}"
        );
    }

    // Check that raw cachegrind outputs are saved.
    assert!(fs::read_dir(&target_path).unwrap().count() > 0);

    // Check processed outputs.
    let outputs = read_outputs(&out_path);
    assert_initial_outputs(&outputs);

    // Re-run a bench and check that the outputs are consistent.
    let output = Command::new(EXE_PATH)
        .args(["--bench", "--exact", "fib_short"])
        .env(EXPORTER_OUTPUT_VAR, &out_path)
        .env("CACHEGRIND_OUT_DIR", &target_path)
        .output()
        .expect("failed running benches");
    assert!(output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    let benchmark_names: HashSet<_> = stderr
        .lines()
        .filter_map(|line| {
            line.strip_prefix("Benchmarking ")?
                .split_whitespace()
                .next()
        })
        .collect();
    assert_eq!(benchmark_names, HashSet::from(["fib_short"]));

    let new_outputs = read_outputs(&out_path);
    assert_new_outputs(&new_outputs, &outputs);
}

fn assert_initial_outputs(outputs: &HashMap<String, BenchmarkOutput>) {
    let bench_ids: HashSet<_> = outputs.keys().map(String::as_str).collect();
    for &expected_id in EXPECTED_BENCH_NAMES {
        assert!(bench_ids.contains(expected_id), "{bench_ids:?}");
    }

    for output in outputs.values() {
        let stats = output.stats.as_full().unwrap();
        assert!(stats.instructions.total > 0, "{output:?}");
        assert!(stats.data_reads.total > 0, "{output:?}");
        assert!(stats.data_writes.total > 0, "{output:?}");

        let access = AccessSummary::from(*stats);
        assert!(access.instructions > 0, "{access:?}");
        assert!(access.l1_hits > 0, "{access:?}");

        assert!(output.prev_stats.is_none());
    }

    let short_stats = &outputs["fib_short"].stats;
    let long_stats = &outputs["fib_long"].stats;
    assert!(
        long_stats.total_instructions() > 10 * short_stats.total_instructions(),
        "long={long_stats:?}, short={short_stats:?}"
    );
    let guard_stats = &outputs["fib_guard"].stats;
    assert!(
        long_stats.total_instructions() > 10 * guard_stats.total_instructions(),
        "guard={guard_stats:?}, long={long_stats:?}"
    );

    let long_random_walk_stats = &outputs["random_walk/10000000"].stats;
    let long_random_walk_stats = long_random_walk_stats.as_full().unwrap();
    let long_random_walk_output = AccessSummary::from(*long_random_walk_stats);
    assert!(long_random_walk_output.ram_accesses > 1_000);
}

fn assert_new_outputs(
    outputs: &HashMap<String, BenchmarkOutput>,
    old: &HashMap<String, BenchmarkOutput>,
) {
    assert_eq!(outputs.len(), 1);
    let short_output = &outputs["fib_short"];
    let expected_old_stats = old["fib_short"].stats;
    assert_eq!(short_output.prev_stats, Some(expected_old_stats));
    assert_close(
        short_output.stats.as_full().unwrap(),
        expected_old_stats.as_full().unwrap(),
    );
}

#[test]
fn printing_benchmark_results() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let out_path = temp_dir.path().join("out.json");
    let target_path = temp_dir.path().join("target");

    let exit_status = Command::new(EXE_PATH)
        .args(["--bench", "fib_"])
        .env("CACHEGRIND_OUT_DIR", &target_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("failed running benches");
    assert!(exit_status.success());

    let output = Command::new(EXE_PATH)
        .args(["--bench", "--print"])
        .env(EXPORTER_OUTPUT_VAR, &out_path)
        .env("CACHEGRIND_OUT_DIR", &target_path)
        .output()
        .expect("failed running benches");
    assert!(output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    let benchmark_names: HashSet<_> = stderr
        .lines()
        .filter_map(|line| {
            line.strip_prefix("Benchmarking ")?
                .split_whitespace()
                .next()
        })
        .collect();
    for &name in EXPECTED_BENCH_NAMES {
        assert!(
            benchmark_names.contains(name),
            "{benchmark_names:?} doesn't contain {name}"
        );
    }

    let benchmarks_without_data = stderr
        .lines()
        .filter(|line| line.ends_with("no data"))
        .count();
    assert_eq!(benchmarks_without_data, 5); // `fib/` and `random_walk/` benches

    // Check that only outputs for benches that have already been run are supplied to the processor.
    let outputs = read_outputs(&out_path);
    assert!(
        outputs.keys().all(|id| id.starts_with("fib_")),
        "{outputs:?}"
    );
    assert!(
        outputs.values().all(|output| output.prev_stats.is_none()),
        "{outputs:?}"
    );
}
