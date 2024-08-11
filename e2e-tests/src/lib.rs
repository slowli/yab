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

pub fn main() {
    let mut bencher = Bencher::default();
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
