use yab::{black_box, Bencher, BenchmarkId};

fn fibonacci(n: u64) -> u64 {
    match n {
        0 => 1,
        1 => 1,
        n => fibonacci(n - 1) + fibonacci(n - 2),
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
}
