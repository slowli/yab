// Linter settings.
#![warn(missing_debug_implementations, bare_trait_objects)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(
    clippy::must_use_candidate,
    clippy::module_name_repetitions,
    clippy::missing_panics_doc
)]

use std::{
    collections::HashSet,
    hash::{BuildHasherDefault, DefaultHasher},
};

use rand::{rngs::SmallRng, Rng, SeedableRng};
use yab::{black_box, captures, Bencher, BenchmarkId};

use crate::exporter::BenchmarkExporter;
pub use crate::exporter::EXPORTER_OUTPUT_VAR;

mod exporter;

type ZeroHasher = BuildHasherDefault<DefaultHasher>;

const RNG_SEED: u64 = 123;

fn fibonacci(n: u64) -> u64 {
    match n {
        0 | 1 => 1,
        n => fibonacci(n - 1) + fibonacci(n - 2),
    }
}

fn bench_rng<const LEN: usize>(bencher: &mut Bencher) {
    bencher.bench_with_captures(
        BenchmarkId::new("rng", LEN),
        captures!(|[outer, gen_in_loop, gen_array]| {
            let mut rng = SmallRng::seed_from_u64(RNG_SEED);
            outer.measure(|| {
                gen_in_loop.measure(|| {
                    for _ in 0..LEN {
                        black_box(rng.random::<u64>());
                    }
                });
                gen_array.measure(|| rng.random::<[u64; LEN]>());
            });
        }),
    );
}

struct FibGuard(u64);

impl Drop for FibGuard {
    fn drop(&mut self) {
        fibonacci(black_box(self.0));
    }
}

pub fn main(bencher: &mut Bencher) {
    bencher.add_reporter(BenchmarkExporter::default());
    bencher
        .bench("fib_short", || fibonacci(black_box(10)))
        .bench("fib_long", || fibonacci(black_box(30)));
    for n in [15, 20, 25] {
        let id = BenchmarkId::new("fib", n);
        bencher.bench(id, || fibonacci(black_box(n)));
    }

    bencher.bench_with_capture("fib_capture", |capture| {
        black_box(fibonacci(black_box(30)));
        let output = capture.measure(|| fibonacci(black_box(10)));
        assert_eq!(output, 89);
    });

    // Dropping the guard should not be measured
    bencher.bench("guard", || {
        fibonacci(black_box(10));
        FibGuard(20)
    });
    bencher.bench_with_capture("guard/explicit", |capture| {
        capture.measure(|| {
            fibonacci(black_box(10));
            FibGuard(20)
        });
    });

    let mut rng = SmallRng::seed_from_u64(RNG_SEED);
    let random_bytes: Vec<u64> = (0..10_000_000).map(|_| rng.random()).collect();

    for len in [1_000_000, 10_000_000] {
        let id = BenchmarkId::new("random_walk", len);
        bencher.bench(id, || {
            let random_bytes = black_box(&random_bytes[..len]);
            let mut pos = 0_usize;

            #[allow(clippy::cast_possible_truncation)]
            for _ in 0..100_000 {
                pos = black_box(
                    pos.wrapping_mul(31)
                        .wrapping_add(random_bytes[black_box(pos) % len] as usize),
                );
            }
            pos
        });
    }

    bencher.bench_with_captures(
        "hash_set",
        captures!(|[collect, sum, drain]| {
            // Use a deterministic (zero) seed for the hasher to get reproducible results.
            let mut rng = SmallRng::seed_from_u64(RNG_SEED);
            let set: HashSet<u64, ZeroHasher> =
                collect.measure(|| (0..10_000).map(|_| rng.random()).collect());
            sum.measure(|| set.iter().copied().reduce(u64::wrapping_add));
            drain.measure(|| {
                for item in set {
                    black_box(item);
                }
            });
        }),
    );

    bench_rng::<10_000>(bencher);
}
