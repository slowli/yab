use std::{collections::HashMap, env, fs, io, sync::Mutex};

use yab::{BenchmarkId, BenchmarkOutput, BenchmarkProcessor};

pub const EXPORTER_OUTPUT_VAR: &str = "YAB_BENCHMARKS_JSON";

#[derive(Debug, Default)]
pub(crate) struct BenchmarkExporter {
    outputs: Mutex<HashMap<String, BenchmarkOutput>>,
}

impl BenchmarkProcessor for BenchmarkExporter {
    fn process_benchmark(&self, id: &BenchmarkId, output: BenchmarkOutput) {
        self.outputs.lock().unwrap().insert(id.to_string(), output);
    }
}

impl Drop for BenchmarkExporter {
    fn drop(&mut self) {
        let Ok(out_path) = env::var(EXPORTER_OUTPUT_VAR) else {
            return;
        };
        let out_file = fs::File::create(&out_path).unwrap_or_else(|err| {
            panic!("Failed writing outputs to `{out_path}`: {err}");
        });
        let out_file = io::BufWriter::new(out_file);
        let outputs = Mutex::get_mut(&mut self.outputs).unwrap();
        serde_json::to_writer_pretty(out_file, outputs).expect("failed exporting results");
    }
}
