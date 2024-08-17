use rand::{rngs::SmallRng, Rng, SeedableRng};
use yab::{black_box, Bencher, BenchmarkId};

use crate::exporter::BenchmarkExporter;
pub use crate::exporter::EXPORTER_OUTPUT_VAR;

mod exporter;

const RNG_SEED: u64 = 123;

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
    let mut bencher = Bencher::default().with_processor(BenchmarkExporter::default());
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

    let mut rng = SmallRng::seed_from_u64(RNG_SEED);
    let random_bytes: Vec<usize> = (0..10_000_000).map(|_| rng.gen()).collect();

    for len in [1_000_000, 10_000_000] {
        let id = BenchmarkId::new("random_walk", len);
        bencher.bench(id, || {
            let random_bytes = black_box(&random_bytes[..len]);
            let mut pos = 0_usize;
            for _ in 0..100_000 {
                pos = black_box(
                    pos.wrapping_mul(31)
                        .wrapping_add(random_bytes[black_box(pos) % len]),
                );
            }
            pos
        });
    }
}
