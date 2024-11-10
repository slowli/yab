//! Mock cachegrind wrapper.

use std::{
    collections::HashMap,
    env, fs,
    io::{self, Write as _},
    thread,
    time::Duration,
};

use serde::Deserialize;
use yab::{CachegrindDataPoint, FullCachegrindStats};

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

#[derive(Debug, Deserialize)]
struct AllStats {
    default: HashMap<String, FullCachegrindStats>,
    #[serde(flatten)]
    other_profiles: HashMap<String, HashMap<String, FullCachegrindStats>>,
}

impl AllStats {
    fn get(&self, bench_name: &str, profile: Option<&str>) -> &FullCachegrindStats {
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

    let stats: AllStats = serde_json::from_str(include_str!("all-stats.json"))
        .expect("cannot deserialize sample stats");
    let bench_stats = *stats.get(bench_name, profile.as_deref());

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
