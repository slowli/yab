use std::{
    collections::HashMap,
    env, fs, io,
    sync::{Arc, Mutex},
};

use yab::{
    reporter::{BenchmarkOutput, BenchmarkReporter, Reporter},
    BenchmarkId,
};

pub const EXPORTER_OUTPUT_VAR: &str = "YAB_BENCHMARKS_JSON";

type SharedOutputs = Arc<Mutex<HashMap<String, BenchmarkOutput>>>;

#[derive(Debug, Default)]
pub(crate) struct BenchmarkExporter {
    outputs: SharedOutputs,
}

impl Reporter for BenchmarkExporter {
    fn new_benchmark(&mut self, id: &BenchmarkId) -> Box<dyn BenchmarkReporter> {
        #[derive(Debug)]
        struct Entry(SharedOutputs, String);

        impl BenchmarkReporter for Entry {
            fn ok(self: Box<Self>, output: &BenchmarkOutput) {
                let Self(outputs, id) = *self;
                outputs.lock().unwrap().insert(id, output.clone());
            }
        }

        Box::new(Entry(self.outputs.clone(), id.to_string()))
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
        let outputs = self.outputs.lock().unwrap();
        serde_json::to_writer_pretty(out_file, &*outputs).expect("failed exporting results");
    }
}
