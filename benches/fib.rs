use yab::{black_box, Bencher, BenchmarkId};

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

fn main() {
    let mut bencher = Bencher::default();
    bencher
        .bench_function("fib_short", || fibonacci(black_box(10)))
        .bench_function("fib_long", || fibonacci(black_box(30)));
    for n in [15, 20, 25] {
        let id = BenchmarkId::new("fib", n);
        bencher.bench_function(id, || fibonacci(black_box(n)));
    }

    // Dropping the guard should not be measured
    bencher.bench_function("fib_guard", || FibGuard(30));
}
