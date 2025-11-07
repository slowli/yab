//! Sequential reporter implementation.

use std::{any::Any, mem, sync::Arc};

use super::{BenchmarkOutput, BenchmarkReporter, Logger, Reporter, TestReporter};
use crate::{BenchmarkId, CachegrindStats};

#[derive(Debug)]
pub(crate) struct SeqReporter {
    reporters: Vec<Box<dyn Reporter>>,
    pub(crate) logger: Arc<dyn Logger>,
}

impl SeqReporter {
    pub fn new(logger: Arc<dyn Logger>) -> Self {
        Self {
            reporters: vec![],
            logger,
        }
    }

    pub fn push(&mut self, mut reporter: Box<dyn Reporter>) {
        reporter.set_logger(&self.logger);
        self.reporters.push(reporter);
    }

    pub fn ok_all(&mut self) {
        for reporter in mem::take(&mut self.reporters) {
            reporter.ok();
        }
    }
}

impl Reporter for SeqReporter {
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

        let reporters = self
            .reporters
            .iter_mut()
            .map(|reporter| reporter.new_test(id));
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
        }

        let reporters = self
            .reporters
            .iter_mut()
            .map(|reporter| reporter.new_benchmark(id));
        Box::new(Seq(reporters.collect()))
    }
}
