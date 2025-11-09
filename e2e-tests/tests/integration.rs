//! Should preferably run in the release mode to emulate real benchmark env, and because some benches
//! are quite slow otherwise.

use std::{
    collections::{HashMap, HashSet},
    env, fs, io,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use once_cell::sync::Lazy;
use yab::{
    reporter::BenchmarkOutput, AccessSummary, CachegrindOutput, CachegrindStats,
    FullCachegrindStats,
};
use yab_e2e_tests::EXPORTER_OUTPUT_VAR;

const EXE_PATH: &str = env!("CARGO_BIN_EXE_yab-e2e-tests");
const MOCK_CACHEGRIND_PATH: &str = env!("CARGO_BIN_EXE_mock-cachegrind");

type Baseline = HashMap<String, CachegrindOutput>;

// Because benchmarked functions are simple, hopefully the snapshot won't depend much on architecture,
// Rust compiler version etc.
static EXPECTED_STATS: Lazy<Baseline> =
    Lazy::new(|| serde_json::from_str(include_str!("../benches/all/main.baseline.json")).unwrap());

const EXPECTED_BENCH_NAMES: &[&str] = &[
    "fib_short",
    "fib_long",
    "fib/15",
    "fib/20",
    "fib/25",
    "fib_capture",
    "guard",
    "random_walk/1000000",
    "random_walk/10000000",
];

fn read_outputs(path: &Path) -> HashMap<String, BenchmarkOutput> {
    let reader = fs::File::open(path).unwrap();
    serde_json::from_reader(io::BufReader::new(reader)).unwrap()
}

fn assert_close(actual: &FullCachegrindStats, expected: &FullCachegrindStats) {
    let points = [
        (actual.instructions, expected.instructions),
        (actual.data_reads, expected.data_reads),
        (actual.data_writes, expected.data_writes),
    ];
    for (new_point, old_point) in points {
        assert_close_values(new_point.total, old_point.total);
        assert_close_values(new_point.l1_misses, old_point.l1_misses);
        assert_close_values(new_point.l3_misses, old_point.l3_misses);
    }
}

fn assert_close_values(actual: u64, expected: u64) {
    let threshold = (expected / 50).max(50); // allow divergence up to 2%, and more for smaller values
    let diff = actual.abs_diff(expected);
    assert!(diff <= threshold, "actual={actual}, expected={expected}");
}

#[test]
fn testing_benchmarks() {
    // Without `--bench` argument, benches should be tested.
    let output = Command::new(EXE_PATH).output().unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");
    assert!(!stderr.contains('\u{1b}')); // no ANSI escape sequences since stderr is not a TTY

    let test_names: HashSet<_> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("[√] ")?.split_whitespace().next())
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
        .filter_map(|line| line.strip_prefix("[√] ")?.split_whitespace().next())
        .collect();
    assert_eq!(test_names, HashSet::from(["fib/15", "fib/20", "fib/25"]));
}

#[test]
fn testing_with_regex_filter() {
    let output = Command::new(EXE_PATH).arg("/\\d+$").output().unwrap();
    assert!(output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    let test_names: HashSet<_> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("[√] ")?.split_whitespace().next())
        .collect();
    assert_eq!(
        test_names,
        HashSet::from([
            "fib/15",
            "fib/20",
            "fib/25",
            "random_walk/1000000",
            "random_walk/10000000"
        ])
    );
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
        .filter_map(|line| line.strip_prefix("[√] ")?.split_whitespace().next())
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
        .filter_map(|line| line.strip_prefix("[√] ")?.split_whitespace().next())
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
        let stats = output.stats.summary.as_full().unwrap();
        assert!(stats.instructions.total > 0, "{stats:?}");
        assert!(stats.data_reads.total > 0, "{stats:?}");
        assert!(stats.data_writes.total > 0, "{stats:?}");

        let access = AccessSummary::from(*stats);
        assert!(access.instructions > 0, "{access:?}");
        assert!(access.l1_hits > 0, "{access:?}");

        assert!(output.prev_stats.is_none());
    }

    let short_stats = &outputs["fib_short"].stats.summary;
    let long_stats = &outputs["fib_long"].stats.summary;
    assert!(
        long_stats.total_instructions() > 10 * short_stats.total_instructions(),
        "long={long_stats:?}, short={short_stats:?}"
    );
    let guard_stats = &outputs["guard"].stats.summary;
    assert!(
        long_stats.total_instructions() > 10 * guard_stats.total_instructions(),
        "guard={guard_stats:?}, long={long_stats:?}"
    );

    let long_random_walk_stats = &outputs["random_walk/10000000"].stats.summary;
    let long_random_walk_stats = long_random_walk_stats.as_full().unwrap();
    let long_random_walk_output = AccessSummary::from(*long_random_walk_stats);
    assert!(long_random_walk_output.ram_accesses > 1_000);

    assert_reference_stats(outputs, true);
}

fn assert_reference_stats(outputs: &HashMap<String, BenchmarkOutput>, full: bool) {
    if cfg!(debug_assertions) {
        return;
    }

    let should_skip_complex_stats = env::var("YAB_SKIP_COMPLEX_STATS").is_ok();
    for (name, expected_stats) in &*EXPECTED_STATS {
        let expected_stats = expected_stats.summary.as_full().unwrap();
        if name == "hash_set/collect" && should_skip_complex_stats {
            continue;
        }
        println!("Comparing bench {name}");
        let summary = &outputs[name].stats.summary;
        if full {
            let actual_stats = summary.as_full().unwrap();
            assert_close(actual_stats, expected_stats);
        } else {
            assert!(summary.as_full().is_none());
            let expected_instructions = expected_stats.instructions.total;
            let actual_instructions = summary.total_instructions();
            assert_close_values(actual_instructions, expected_instructions);
        }
    }
}

fn assert_new_outputs(
    outputs: &HashMap<String, BenchmarkOutput>,
    old: &HashMap<String, BenchmarkOutput>,
) {
    assert_eq!(outputs.len(), 1);
    let short_output = &outputs["fib_short"];
    let expected_old_stats = old["fib_short"].stats.summary;
    assert_eq!(
        short_output.prev_stats.as_ref().unwrap().summary,
        expected_old_stats
    );
    assert_close(
        short_output.stats.summary.as_full().unwrap(),
        expected_old_stats.as_full().unwrap(),
    );
}

#[test]
fn benchmarking_everything_with_mock_cachegrind() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let out_path = temp_dir.path().join("out.json");
    let target_path = temp_dir.path().join("target");

    let output = Command::new(EXE_PATH)
        .arg("--bench")
        .env(EXPORTER_OUTPUT_VAR, &out_path)
        .env("CACHEGRIND_WRAPPER", MOCK_CACHEGRIND_PATH)
        .env("CACHEGRIND_OUT_DIR", &target_path)
        .output()
        .expect("failed running benches");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");

    let outputs = read_outputs(&out_path);
    // Check that outputs exactly match the sampled ones
    for (name, expected_stats) in &*EXPECTED_STATS {
        let expected_stats = expected_stats.summary.as_full().unwrap();
        let actual_stats = outputs[name].stats.summary.as_full().unwrap();
        assert_eq!(actual_stats, expected_stats);
    }

    test_handling_interrupts(&temp_dir);
}

fn test_handling_interrupts(temp_dir: &tempfile::TempDir) {
    let out_path = temp_dir.path().join("out.json");
    let target_path = temp_dir.path().join("target");

    let mock_cachegrind = format!("{MOCK_CACHEGRIND_PATH}:--emulate-hang-up");
    let mut output = Command::new(EXE_PATH)
        .arg("--bench")
        .env(EXPORTER_OUTPUT_VAR, &out_path)
        .env("CACHEGRIND_WRAPPER", &mock_cachegrind)
        .env("CACHEGRIND_OUT_DIR", &target_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed spawning bench");

    // Check that the benches have hanged up.
    thread::sleep(Duration::from_secs(1));
    assert!(
        output.try_wait().unwrap().is_none(),
        "benches did not hang up"
    );
    output.kill().unwrap();

    // Print the bench results and check that there's no garbage.
    let output = Command::new(EXE_PATH)
        .arg("--print")
        .env(EXPORTER_OUTPUT_VAR, &out_path)
        .env("CACHEGRIND_WRAPPER", MOCK_CACHEGRIND_PATH)
        .env("CACHEGRIND_OUT_DIR", &target_path)
        .output()
        .expect("failed running benches");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");

    let outputs = read_outputs(&out_path);
    for (name, expected_stats) in &*EXPECTED_STATS {
        let expected_stats = expected_stats.summary.as_full().unwrap();
        let actual_stats = outputs[name].stats.summary.as_full().unwrap();
        assert_eq!(actual_stats, expected_stats);
    }
}

#[test]
fn benchmarking_with_mock_cachegrind_and_custom_profile() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let out_path = temp_dir.path().join("out.json");
    let target_path = temp_dir.path().join("target");

    let mock_cachegrind = format!("{MOCK_CACHEGRIND_PATH}:--profile=cmp");
    let output = Command::new(EXE_PATH)
        .args(["--bench", "fib_short"])
        .env(EXPORTER_OUTPUT_VAR, &out_path)
        .env("CACHEGRIND_WRAPPER", &mock_cachegrind)
        .env("CACHEGRIND_OUT_DIR", &target_path)
        .output()
        .expect("failed running benches");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");

    let outputs = read_outputs(&out_path);
    assert_eq!(
        outputs["fib_short"].stats.summary.total_instructions(),
        1_739
    );
}

#[test]
fn handling_panics_in_benches() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let target_path = temp_dir.path().join("target");

    let mock_cachegrind = format!("{MOCK_CACHEGRIND_PATH}:--emulate-panic");
    let output = Command::new(EXE_PATH)
        .arg("--bench")
        .env("CACHEGRIND_WRAPPER", &mock_cachegrind)
        .env("CACHEGRIND_OUT_DIR", &target_path)
        .output()
        .expect("failed running benches");

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!output.status.success(), "{stderr}");

    // Check that `stderr` contains actionable output.
    assert!(stderr.contains("cachegrind exited abnormally"), "{stderr}");
    assert!(stderr.contains("panicked at"), "{stderr}");
    assert!(stderr.contains("emulated panic!"), "{stderr}");
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
    let benchmarks_without_data = stderr
        .lines()
        .filter(|line| line.contains("no data for benchmark"))
        .count();
    assert_eq!(benchmarks_without_data, 13); // `fib/`, `guard`, `random_walk/` and capture benches

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

#[test]
fn filtering_by_capture() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let target_path = temp_dir.path().join("target");
    let output = Command::new(EXE_PATH)
        .args(["--bench", "-q", "--print=pub:main", "/(gen|sum)"])
        .env("CACHEGRIND_OUT_DIR", &target_path)
        .output()
        .expect("failed running benches");
    assert!(output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    let benchmark_names: HashSet<_> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("[√] ")?.split_whitespace().next())
        .collect();
    assert_eq!(
        benchmark_names,
        HashSet::from([
            "rng/10000/gen_in_loop",
            "rng/10000/gen_array",
            "hash_set/sum",
        ])
    );
}

#[test]
fn using_exact_match_with_capture() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let target_path = temp_dir.path().join("target");
    let output = Command::new(EXE_PATH)
        .args(["--bench", "--print=pub:main", "--exact", "hash_set/sum"])
        .env("CACHEGRIND_OUT_DIR", &target_path)
        .output()
        .expect("failed running benches");
    assert!(output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    let benchmark_names: HashSet<_> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("[√] ")?.split_whitespace().next())
        .collect();
    assert_eq!(benchmark_names, HashSet::from(["hash_set/sum"]));
}

#[test]
fn using_custom_job_count() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let out_path = temp_dir.path().join("out.json");
    let target_path = temp_dir.path().join("target");

    let status = Command::new(EXE_PATH)
        .args(["--bench", "fib"])
        .env(EXPORTER_OUTPUT_VAR, &out_path)
        .env("CACHEGRIND_OUT_DIR", &target_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("failed running benches");
    assert!(status.success());

    let initial_outputs = read_outputs(&out_path);

    for jobs in [1, 3] {
        let status = Command::new(EXE_PATH)
            .args(["--jobs", &jobs.to_string(), "--bench", "fib"])
            .env(EXPORTER_OUTPUT_VAR, &out_path)
            .env("CACHEGRIND_OUT_DIR", &target_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("failed running benches");
        assert!(status.success());

        let outputs = read_outputs(&out_path);
        for (name, output) in outputs {
            println!("Comparing bench {name}");
            let stats = output.stats.summary.as_full().unwrap();
            let initial_stats = &initial_outputs[&name].stats.summary;
            let initial_stats = initial_stats.as_full().unwrap();
            assert_close(stats, initial_stats);
        }
    }
}

#[test]
fn disabling_cache_simulation() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let out_path = temp_dir.path().join("out.json");
    let target_path = temp_dir.path().join("target");

    let output = Command::new(EXE_PATH)
        .args([
            "--cg=valgrind",
            "--cg=--tool=cachegrind",
            "--cg=--cache-sim=no",
            "--bench",
        ])
        .env(EXPORTER_OUTPUT_VAR, &out_path)
        .env("CACHEGRIND_OUT_DIR", &target_path)
        .output()
        .expect("failed running benches");
    assert!(output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    let benchmark_names: HashSet<_> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("[√] ")?.split_whitespace().next())
        .collect();
    for &name in EXPECTED_BENCH_NAMES {
        assert!(
            benchmark_names.contains(name),
            "{benchmark_names:?} doesn't contain {name}"
        );
    }

    let outputs = read_outputs(&out_path);
    for &name in EXPECTED_BENCH_NAMES {
        assert!(outputs[name].prev_stats.is_none());
        let stats = outputs[name].stats.summary;
        if let CachegrindStats::Simple { instructions, .. } = stats {
            assert!(instructions > 100);
        } else {
            panic!("Unexpected stats: {stats:?}");
        }
    }
    let short_instructions = outputs["fib_short"].stats.summary.total_instructions();
    let long_instructions = outputs["fib_long"].stats.summary.total_instructions();
    assert!(
        long_instructions > 10 * short_instructions,
        "short={short_instructions}, long={long_instructions}"
    );

    let guard_instructions = outputs["guard"].stats.summary.total_instructions();
    assert!(
        guard_instructions.abs_diff(short_instructions) < 10,
        "short={short_instructions}, guard={guard_instructions}"
    );

    assert_reference_stats(&outputs, false);
}

#[test]
fn printing_public_baseline() {
    let output = Command::new(EXE_PATH)
        .arg("--print=pub:main")
        .output()
        .expect("failed running benches");
    assert!(output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    let benchmark_names: HashSet<_> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("[√] "))
        .collect();
    for &name in EXPECTED_BENCH_NAMES {
        assert!(
            benchmark_names.contains(name),
            "{benchmark_names:?} doesn't contain {name}"
        );
    }
}

#[test]
fn comparing_public_baselines() {
    let output = Command::new(EXE_PATH)
        .args(["--print=pub:main", "--vs=pub:cmp", "random_walk"])
        .output()
        .expect("failed running benches");
    assert!(output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    let mut lines = stderr.lines();
    assert!(
        lines.any(|line| line == "├ Instructions           1800019       +30008 (+1.70%)"),
        "{stderr}"
    );
}

#[test]
fn comparing_public_baselines_with_threshold() {
    let output = Command::new(EXE_PATH)
        .args([
            "--print=pub:main",
            "--vs=pub:cmp",
            "--threshold=0.01",
            "random_walk",
        ])
        .output()
        .expect("failed running benches");
    assert!(!output.status.success());

    let stderr = String::from_utf8(output.stderr).unwrap();
    let warn = stderr
        .lines()
        .find_map(|line| line.strip_prefix(" WARN: "))
        .unwrap();
    assert!(
        warn.contains("random_walk/10000000") && warn.contains("bench has regressed by 1.7%"),
        "{stderr}"
    );

    let error = stderr
        .lines()
        .find_map(|line| line.strip_prefix("ERROR: "))
        .unwrap();
    assert_eq!(error, "1 bench has regressed by >1.0%:");
}

#[test]
fn threshold_is_ignored_in_test_mode() {
    let output = Command::new(EXE_PATH)
        .env("CACHEGRIND_REGRESSION_THRESHOLD", "1%")
        .output()
        .expect("failed running benches");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");
}
