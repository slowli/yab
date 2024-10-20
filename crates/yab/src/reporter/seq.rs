//! Sequential reporter implementation.

use std::{any::Any, fmt::Display};

use super::{BenchmarkOutput, BenchmarkReporter, Reporter, TestReporter};
use crate::{BenchmarkId, CachegrindStats};

#[derive(Debug, Default)]
pub(crate) struct SeqReporter(pub Vec<Box<dyn Reporter>>);

impl SeqReporter {
    pub fn ok_all(self) {
        for reporter in self.0 {
            reporter.ok();
        }
    }
}

impl Reporter for SeqReporter {
    fn error(&mut self, error: &dyn Display) {
        for reporter in &mut self.0 {
            reporter.error(error);
        }
    }

    fn new_test(&mut self, id: &BenchmarkId) -> Box<dyn TestReporter> {
        struct Seq(Vec<Box<dyn TestReporter>>);

        impl TestReporter for Seq {
            fn ok(self: Box<Self>) {
                for reporter in self.0 {
                    reporter.ok();
                }
            }

            fn fail(self: Box<Self>, panic_data: &dyn Any) {
                for reporter in self.0 {
                    reporter.fail(panic_data);
                }
            }
        }

        let reporters = self.0.iter_mut().map(|reporter| reporter.new_test(id));
        Box::new(Seq(reporters.collect()))
    }

    fn new_benchmark(&mut self, id: &BenchmarkId) -> Box<dyn BenchmarkReporter> {
        #[derive(Debug)]
        struct Seq(Vec<Box<dyn BenchmarkReporter>>);

        impl BenchmarkReporter for Seq {
            fn start_execution(&mut self) {
                for reporter in &mut self.0 {
                    reporter.start_execution();
                }
            }

            fn baseline_computed(&mut self, stats: &CachegrindStats) {
                for reporter in &mut self.0 {
                    reporter.baseline_computed(stats);
                }
            }

            fn ok(self: Box<Self>, output: &BenchmarkOutput) {
                for reporter in self.0 {
                    reporter.ok(output);
                }
            }

            fn warning(&mut self, warning: &dyn Display) {
                for reporter in &mut self.0 {
                    reporter.warning(warning);
                }
            }

            fn error(self: Box<Self>, error: &dyn Display) {
                for reporter in self.0 {
                    reporter.error(error);
                }
            }
        }

        let reporters = self.0.iter_mut().map(|reporter| reporter.new_benchmark(id));
        Box::new(Seq(reporters.collect()))
    }
}
