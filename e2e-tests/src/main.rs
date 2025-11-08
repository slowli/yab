fn main() {
    // Manually override the benchmark name.
    yab_e2e_tests::main(&mut yab::Bencher::new("all"));
}
