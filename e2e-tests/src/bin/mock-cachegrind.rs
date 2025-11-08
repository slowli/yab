//! Mock cachegrind wrapper.
//!
//! To update stats in `all-stats.json`, run `cargo bench --bench all` with the `YAB_BENCHMARKS_JSON` env var set
//! (e.g., to `stats.json`). The stats will be output to the specified location.

use std::{
    collections::HashMap,
    env, fs,
    io::{self, Write as _},
    thread,
    time::Duration,
};

use yab::{CachegrindDataPoint, CachegrindOutput, FullCachegrindStats};

const CONST_OVERHEAD: FullCachegrindStats = FullCachegrindStats {
    instructions: CachegrindDataPoint {
        total: 1_000,
        l1_misses: 50,
        l3_misses: 0,
    },
    data_reads: CachegrindDataPoint {
        total: 250,
        l1_misses: 50,
        l3_misses: 0,
    },
    data_writes: CachegrindDataPoint {
        total: 100,
        l1_misses: 10,
        l3_misses: 0,
    },
};

const ITER_OVERHEAD: FullCachegrindStats = FullCachegrindStats {
    instructions: CachegrindDataPoint {
        total: 100,
        l1_misses: 10,
        l3_misses: 0,
    },
    data_reads: CachegrindDataPoint {
        total: 25,
        l1_misses: 5,
        l3_misses: 0,
    },
    data_writes: CachegrindDataPoint {
        total: 10,
        l1_misses: 0,
        l3_misses: 0,
    },
};

type Baseline = HashMap<String, CachegrindOutput>;

macro_rules! include_baseline {
    ($name:tt) => {
        serde_json::from_str::<Baseline>(include_str!(concat!(
            "../../benches/all/",
            $name,
            ".baseline.json"
        )))
        .expect(concat!("failed parsing baseline ", $name))
    };
}

#[derive(Debug)]
struct AllStats {
    default: Baseline,
    breakdown: Baseline,
    other_profiles: HashMap<String, Baseline>,
}

impl AllStats {
    fn load() -> Self {
        Self {
            default: include_baseline!("main"),
            breakdown: include_baseline!("breakdown"),
            other_profiles: HashMap::from([("cmp".to_owned(), include_baseline!("cmp"))]),
        }
    }

    fn get(&self, bench_name: &str, profile: Option<&str>) -> &CachegrindOutput {
        let profile_stats = profile.and_then(|profile| {
            let profile = self.other_profiles.get(profile).unwrap_or_else(|| {
                panic!("Profile `{profile}` is undefined");
            });
            profile.get(bench_name)
        });
        profile_stats.unwrap_or_else(|| {
            self.default.get(bench_name).unwrap_or_else(|| {
                panic!("Unexpected bench name: {bench_name}");
            })
        })
    }
}

fn main() {
    let emulate_panic = env::args().any(|arg| arg == "--emulate-panic");
    if emulate_panic {
        panic!("emulated panic!");
    }

    let profile = env::args().find_map(|arg| Some(arg.strip_prefix("--profile=")?.to_owned()));

    let mut args = env::args().skip(1);
    let out_file_path =
        args.find_map(|arg| Some(arg.strip_prefix("--cachegrind-out-file=")?.to_owned()));
    let out_file_path = out_file_path.expect("output file is not provided");

    // Args provided to bench binary have rigid structure.
    let args_to_bench_binary: Vec<_> = args.collect();
    assert_eq!(args_to_bench_binary[1], "--cachegrind-instrument");
    let iter_count: u64 = args_to_bench_binary[2]
        .parse()
        .expect("invalid iteration count");
    assert_ne!(iter_count, 0);
    let is_baseline = match args_to_bench_binary[3].as_str() {
        "+" => true,
        "-" => false,
        _ => panic!("unexpected `is_baseline` option"),
    };
    let bench_name = &args_to_bench_binary[4];

    let stats = AllStats::load();
    let bench_stats = *stats
        .get(bench_name, profile.as_deref())
        .summary
        .as_full()
        .unwrap();

    let mut full_stats =
        bench_stats * (iter_count - 1) + CONST_OVERHEAD + ITER_OVERHEAD * iter_count;
    if !is_baseline {
        full_stats = full_stats + bench_stats;
    }

    // This emulates hanging up after collecting initial stats.
    let emulate_hang_up = env::args().any(|arg| arg == "--emulate-hang-up");
    if emulate_hang_up && (iter_count > 2 || !is_baseline) {
        thread::sleep(Duration::MAX);
    }

    let file = fs::File::create(&out_file_path).expect("failed creating output file");
    let mut writer = io::BufWriter::new(file);
    writeln!(&mut writer, "cmd: {}", args_to_bench_binary.join(" ")).unwrap();
    writeln!(
        &mut writer,
        "events: Ir I1mr ILmr Dr D1mr DLmr Dw D1mw DLmw"
    )
    .unwrap();

    if !is_baseline {
        if let Some(stats) = stats.breakdown.get(bench_name) {
            let breakdown = &stats.breakdown;
            let mut functions_by_file = HashMap::<_, Vec<_>>::new();
            for (function, fn_stats) in breakdown {
                let fn_stats = fn_stats.as_full().unwrap();
                functions_by_file
                    .entry(function.filename().unwrap_or("???"))
                    .or_default()
                    .push((function.name(), fn_stats));
            }

            for (filename, functions) in functions_by_file {
                writeln!(&mut writer, "fl={filename}").unwrap();
                for (name, fn_stats) in functions {
                    writeln!(&mut writer, "fn={name}").unwrap();
                    writeln!(
                        &mut writer,
                        "0 {Ir} {I1mr} {ILmr} {Dr} {D1mr} {DLmr} {Dw} {D1mw} {DLmw}",
                        Ir = fn_stats.instructions.total,
                        I1mr = fn_stats.instructions.l1_misses,
                        ILmr = fn_stats.instructions.l3_misses,
                        Dr = fn_stats.data_reads.total,
                        D1mr = fn_stats.data_reads.l1_misses,
                        DLmr = fn_stats.data_reads.l3_misses,
                        Dw = fn_stats.data_writes.total,
                        D1mw = fn_stats.data_writes.l1_misses,
                        DLmw = fn_stats.data_writes.l3_misses
                    )
                    .unwrap();
                }
            }
        }
    }

    writeln!(
        &mut writer,
        "summary: {Ir} {I1mr} {ILmr} {Dr} {D1mr} {DLmr} {Dw} {D1mw} {DLmw}",
        Ir = full_stats.instructions.total,
        I1mr = full_stats.instructions.l1_misses,
        ILmr = full_stats.instructions.l3_misses,
        Dr = full_stats.data_reads.total,
        D1mr = full_stats.data_reads.l1_misses,
        DLmr = full_stats.data_reads.l3_misses,
        Dw = full_stats.data_writes.total,
        D1mw = full_stats.data_writes.l1_misses,
        DLmw = full_stats.data_writes.l3_misses
    )
    .unwrap();
}
