use yab::{black_box, Bencher};

fn fibonacci(n: u64) -> u64 {
    match n {
        0 => 1,
        1 => 1,
        n => fibonacci(n - 1) + fibonacci(n - 2),
    }
}

fn main() {
    Bencher::default()
        .bench_function("fib_short", || fibonacci(black_box(10)))
        .bench_function("fib_long", || fibonacci(black_box(30)));
}
