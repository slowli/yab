use std::{
    collections::{HashMap, HashSet},
    env,
};

use yab::{black_box, AccessSummary, Bencher, BenchmarkId, BenchmarkOutput, BenchmarkProcessor};

fn fibonacci(n: u64) -> u64 {
    match n {
        0 => 1,
        1 => 1,
        n => fibonacci(n - 1) + fibonacci(n - 2),
    }
}

struct FibGuard(u64);

impl Drop for FibGuard {
    fn drop(&mut self) {
        fibonacci(black_box(self.0));
    }
}

#[derive(Debug, Default)]
struct BenchmarkChecker {
    results: HashMap<BenchmarkId, BenchmarkOutput>,
}

impl BenchmarkProcessor for BenchmarkChecker {
    fn process_benchmark(&mut self, id: &BenchmarkId, output: BenchmarkOutput) {
        self.results.insert(id.clone(), output);
    }
}

impl Drop for BenchmarkChecker {
    fn drop(&mut self) {
        let args: Vec<_> = env::args().skip(1).collect();
        if args != ["--bench"] && args != ["--print", "--bench"] {
            return; // Return if filtering is enabled, or we're in test mode
        }

        let bench_ids: HashSet<_> = self.results.keys().map(BenchmarkId::to_string).collect();
        for expected_id in [
            "fib_short",
            "fib_long",
            "fib/15",
            "fib/20",
            "fib/25",
            "fib_guard",
        ] {
            assert!(bench_ids.contains(expected_id), "{bench_ids:?}");
        }

        for output in self.results.values() {
            assert!(output.summary.instructions.total > 0, "{output:?}");
            assert!(output.summary.data_reads.total > 0, "{output:?}");
            assert!(output.summary.data_writes.total > 0, "{output:?}");

            let access = AccessSummary::from(output.summary);
            assert!(access.instructions > 0, "{access:?}");
            assert!(access.l1_hits > 0, "{access:?}");
        }

        let short_output = &self.results[&BenchmarkId::from("fib_short")];
        let long_output = &self.results[&BenchmarkId::from("fib_long")];
        assert!(
            long_output.summary.instructions.total > 10 * short_output.summary.instructions.total,
            "long={long_output:?}, short={short_output:?}"
        );
        let guard_output = &self.results[&BenchmarkId::from("fib_guard")];
        assert!(
            short_output.summary.instructions.total > 10 * guard_output.summary.instructions.total,
            "guard={guard_output:?}, short={short_output:?}"
        );
    }
}

fn main() {
    let mut bencher = Bencher::default().with_processor(BenchmarkChecker::default());
    bencher
        .bench("fib_short", || fibonacci(black_box(10)))
        .bench("fib_long", || fibonacci(black_box(30)));
    for n in [15, 20, 25] {
        let id = BenchmarkId::new("fib", n);
        bencher.bench(id, || fibonacci(black_box(n)));
    }

    bencher.bench_with_setup("fib_setup", |instr| {
        black_box(fibonacci(black_box(30)));
        instr.start();
        fibonacci(black_box(20))
    });

    // Dropping the guard should not be measured
    bencher.bench("fib_guard", || {
        fibonacci(black_box(10));
        FibGuard(20)
    });
}
