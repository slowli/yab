/// Wrapper for a closure that allows to pass it to [`Bencher::bench_with_captures()`](crate::Bencher::bench_with_captures()).
///
/// # Examples
///
/// ```
/// use std::collections::BTreeSet;
/// use yab::captures;
///
/// fn benchmark(bencher: &mut yab::Bencher) {
///     bencher.bench_with_captures(
///         "ordered_set",
///         captures!(|[collect, sum]| {
///             let set: BTreeSet<u64> = collect.measure(|| {
///                 (0..10_000).map(yab::black_box).collect()
///             });
///             let sum = sum.measure(|| set.into_iter().sum::<u64>());
///             assert_eq!(sum, 9_999 * 10_000 / 2);
///         }),
///     );
///     // This will report `ordered_set/collect` and `ordered_set/sum` benchmarks.
/// }
/// # benchmark(&mut yab::Bencher::new("test"));
/// ```
#[macro_export]
macro_rules! captures {
    (|[$($arg:tt),+]| $block:block) => {{
        (
            [$(::core::stringify!($arg),)+],
            |[$($arg,)+]| $block,
        )
    }};
}
