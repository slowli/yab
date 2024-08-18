//! Reporter implementation printing output to `stderr` in human-readable form.

use std::{
    any::Any,
    cmp::Ordering,
    fmt,
    fmt::Display,
    io,
    io::IsTerminal,
    ops::DerefMut,
    sync::{Arc, Mutex},
    time::Instant,
};

use anes::{Attribute, Color, ResetAttributes, SetAttribute, SetForegroundColor};

use super::{BenchmarkOutput, Reporter};
use crate::{
    cachegrind::{AccessSummary, CachegrindStats},
    BenchmarkId,
};

#[derive(Debug)]
struct LinePrinter<W>(W);

impl<W: io::Write> LinePrinter<W> {
    fn println(&mut self, line: &str) {
        writeln!(&mut self.0, "{line}").expect("I/O error writing to stderr");
    }
}

#[derive(Debug)]
pub(crate) struct PrintingReporter<W = io::Stderr> {
    styling: bool,
    line_printer: Arc<Mutex<LinePrinter<W>>>,
}

impl<W> Clone for PrintingReporter<W> {
    fn clone(&self) -> Self {
        Self {
            styling: self.styling,
            line_printer: self.line_printer.clone(),
        }
    }
}

impl Default for PrintingReporter {
    fn default() -> Self {
        let line_printer = LinePrinter(io::stderr());
        Self {
            styling: io::stderr().is_terminal(),
            line_printer: Arc::new(Mutex::new(line_printer)),
        }
    }
}

impl PrintingReporter {
    pub fn report_list_item(id: &BenchmarkId) {
        println!("{id}: benchmark");
    }
}

impl<W: io::Write> PrintingReporter<W> {
    fn lock_printer(&self) -> impl DerefMut<Target = LinePrinter<W>> + '_ {
        self.line_printer.lock().expect("line printer is poisoned")
    }

    fn with_color(&self, message: String, color: Color) -> String {
        if self.styling {
            format!("{}{message}{ResetAttributes}", SetForegroundColor(color))
        } else {
            message
        }
    }

    fn bold(&self, message: String) -> String {
        if self.styling {
            format!(
                "{}{message}{ResetAttributes}",
                SetAttribute(Attribute::Bold)
            )
        } else {
            message
        }
    }

    fn dimmed(&self, message: String) -> String {
        if self.styling {
            format!(
                "{}{message}{ResetAttributes}",
                SetAttribute(Attribute::Faint)
            )
        } else {
            message
        }
    }

    fn format_id(&self, id: &BenchmarkId) -> String {
        let BenchmarkId {
            name,
            args,
            location,
        } = id;
        let args = if let Some(args) = args {
            format!("/{args}")
        } else {
            String::new()
        };
        let location = self.dimmed(format!(" @ {}:{}", location.file(), location.line()));
        format!("{name}{args}{location}")
    }

    #[allow(clippy::cast_precision_loss, clippy::cast_possible_wrap)] // fine for reporting
    fn report_diff(&self, new: u64, old: u64) -> String {
        match new.cmp(&old) {
            Ordering::Less => {
                let diff = format!(
                    "{:>+15} ({:+.2}%)",
                    new as i64 - old as i64,
                    (old - new) as f32 * -100.0 / old as f32
                );
                self.with_color(diff, Color::Green)
            }
            Ordering::Greater => {
                let diff = format!(
                    "{:>+15} ({:+.2}%)",
                    new - old,
                    (new - old) as f32 * 100.0 / old as f32
                );
                self.with_color(diff, Color::Red)
            }
            Ordering::Equal => "    (no change)".to_owned(),
        }
    }

    pub(crate) fn report_error(&self, err: &dyn fmt::Display, id: Option<&str>) {
        let error = self.with_color(self.bold("ERROR:".into()), Color::Red);
        let maybe_id = if let Some(id) = id {
            format!(" {id}:")
        } else {
            String::new()
        };
        self.lock_printer()
            .println(&format!("{error}{maybe_id} {err}"));
    }

    fn report_warning(&self, err: &dyn fmt::Display, id: &str) {
        let warn = self.with_color(self.bold("WARN:".into()), Color::Yellow);
        self.lock_printer().println(&format!("{warn} {id}: {err}"));
    }
}

#[derive(Debug)]
pub(crate) struct TestReporter<W> {
    parent: PrintingReporter<W>,
    test_id: String,
    started_at: Instant,
}

impl<W: io::Write> super::TestReporter for TestReporter<W> {
    fn fail(self: Box<Self>, _: &dyn Any) {
        let id = &self.test_id;
        let failed = self
            .parent
            .with_color(self.parent.bold("FAILED".into()), Color::Red);
        self.parent
            .lock_printer()
            .println(&format!("Testing {id}: {failed}"));
    }

    fn ok(self: Box<Self>) {
        let id = &self.test_id;
        let latency = self.started_at.elapsed();
        let ok = self.parent.bold("OK".into());
        self.parent
            .lock_printer()
            .println(&format!("Testing {id}: {ok} ({latency:?})"));
    }
}

#[derive(Debug)]
struct BenchmarkReporter<W> {
    parent: PrintingReporter<W>,
    bench_id: String,
    started_at: Option<Instant>,
}

impl<W: io::Write> BenchmarkReporter<W> {
    fn instruction_diff(&self, printer: &mut LinePrinter<W>, new: u64, old: Option<u64>) {
        let diff = old
            .map(|old| self.parent.report_diff(new, old))
            .unwrap_or_default();
        printer.println(&format!("  Instructions: {new:>15} {diff}"));
    }

    fn full_diff(
        &self,
        printer: &mut LinePrinter<W>,
        summary: AccessSummary,
        old_summary: Option<AccessSummary>,
    ) {
        let diff = old_summary
            .map(|old| {
                self.parent
                    .report_diff(summary.instructions, old.instructions)
            })
            .unwrap_or_default();
        printer.println(&format!(
            "  Instructions: {:>15} {diff}",
            summary.instructions
        ));

        let diff = old_summary
            .map(|old| self.parent.report_diff(summary.l1_hits, old.l1_hits))
            .unwrap_or_default();
        printer.println(&format!("  L1 hits     : {:>15} {diff}", summary.l1_hits));

        let diff = old_summary
            .map(|old| self.parent.report_diff(summary.l3_hits, old.l3_hits))
            .unwrap_or_default();
        printer.println(&format!("  L2/L3 hits  : {:>15} {diff}", summary.l3_hits));

        let diff = old_summary
            .map(|old| {
                self.parent
                    .report_diff(summary.ram_accesses, old.ram_accesses)
            })
            .unwrap_or_default();
        printer.println(&format!(
            "  RAM accesses: {:>15} {diff}",
            summary.ram_accesses
        ));

        let diff = old_summary
            .map(|old| {
                self.parent
                    .report_diff(summary.estimated_cycles(), old.estimated_cycles())
            })
            .unwrap_or_default();
        printer.println(&format!(
            "  Est. cycles : {:>15} {diff}",
            summary.estimated_cycles()
        ));
    }
}

impl<W: io::Write + fmt::Debug + Send> super::BenchmarkReporter for BenchmarkReporter<W> {
    fn start_execution(&mut self) {
        self.started_at = Some(Instant::now());
    }

    fn baseline_computed(&mut self, stats: &CachegrindStats) {
        let id = &self.bench_id;
        let instr = stats.total_instructions();
        self.parent.lock_printer().println(&format!(
            "Benchmarking {id}: captured baseline ({instr} instructions)"
        ));
    }

    fn ok(self: Box<Self>, output: &BenchmarkOutput) {
        let BenchmarkOutput { stats, prev_stats } = output;
        let mut printer = self.parent.lock_printer();
        let id = &self.bench_id;
        let ok = self.parent.bold("OK".into());
        let latency = if let Some(started_at) = self.started_at {
            let latency = started_at.elapsed();
            self.parent.dimmed(format!(" ({latency:?})"))
        } else {
            String::new()
        };
        printer.println(&format!("Benchmarking {id}: {ok}{latency}"));

        let (stats, prev_stats) = match (*stats, *prev_stats) {
            (CachegrindStats::Simple { instructions }, _) => {
                let old_instructions = prev_stats.as_ref().map(CachegrindStats::total_instructions);
                self.instruction_diff(&mut printer, instructions, old_instructions);
                return;
            }
            (_, Some(CachegrindStats::Simple { instructions: old })) => {
                self.instruction_diff(&mut printer, stats.total_instructions(), Some(old));
                return;
            }
            (CachegrindStats::Full(stats), None) => (stats, None),
            (CachegrindStats::Full(stats), Some(CachegrindStats::Full(old_stats))) => {
                (stats, Some(old_stats))
            }
        };

        self.full_diff(
            &mut printer,
            stats.into(),
            prev_stats.map(AccessSummary::from),
        );
    }

    fn warning(&mut self, warning: &dyn fmt::Display) {
        self.parent.report_warning(warning, &self.bench_id);
    }

    fn error(self: Box<Self>, error: &dyn fmt::Display) {
        self.parent.report_error(error, Some(&self.bench_id));
    }
}

impl<W> Reporter for PrintingReporter<W>
where
    W: io::Write + fmt::Debug + Send + 'static,
{
    fn error(&mut self, error: &dyn Display) {
        self.report_error(error, None);
    }

    fn new_test(&mut self, id: &BenchmarkId) -> Box<dyn super::TestReporter> {
        Box::new(TestReporter {
            parent: self.clone(),
            test_id: self.format_id(id),
            started_at: Instant::now(),
        })
    }

    fn new_benchmark(&mut self, id: &BenchmarkId) -> Box<dyn super::BenchmarkReporter> {
        Box::new(BenchmarkReporter {
            parent: self.clone(),
            bench_id: self.format_id(id),
            started_at: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cachegrind::{CachegrindDataPoint, FullCachegrindStats};

    fn mock_reporter() -> PrintingReporter<Vec<u8>> {
        let line_printer = LinePrinter(vec![]);
        PrintingReporter {
            styling: false,
            line_printer: Arc::new(Mutex::new(line_printer)),
        }
    }

    fn extract_buffer(reporter: PrintingReporter<Vec<u8>>) -> String {
        let buffer = Arc::into_inner(reporter.line_printer).unwrap();
        let buffer = buffer.into_inner().unwrap().0;
        String::from_utf8(buffer).unwrap()
    }

    fn mock_stats() -> FullCachegrindStats {
        FullCachegrindStats {
            instructions: CachegrindDataPoint {
                total: 100,
                l1_misses: 20,
                l3_misses: 10,
            },
            data_reads: CachegrindDataPoint {
                total: 200,
                l1_misses: 40,
                l3_misses: 10,
            },
            data_writes: CachegrindDataPoint {
                total: 50,
                l1_misses: 40,
                l3_misses: 0,
            },
        }
    }

    #[test]
    fn reporting_basic_stats() {
        let mut reporter = mock_reporter();
        let stats = CachegrindStats::Simple { instructions: 123 };
        let mut bench = reporter.new_benchmark(&BenchmarkId::from("test"));
        bench.start_execution();
        bench.ok(&BenchmarkOutput {
            stats,
            prev_stats: None,
        });

        let buffer = extract_buffer(reporter);
        let lines: Vec<_> = buffer.lines().collect();
        assert_eq!(lines.len(), 2, "{buffer}");
        assert!(lines[0].starts_with("Benchmarking test @"), "{buffer}");
        assert!(lines[0].contains("printer.rs"), "{buffer}");
        assert!(lines[0].contains(": OK ("), "{buffer}");
        assert_eq!(lines[1].trim(), "Instructions:             123");
    }

    #[test]
    fn reporting_basic_stats_with_diff() {
        let mut reporter = mock_reporter();
        let stats = CachegrindStats::Simple { instructions: 120 };
        let prev_stats = CachegrindStats::Simple { instructions: 100 };
        reporter
            .new_benchmark(&BenchmarkId::from("test"))
            .ok(&BenchmarkOutput {
                stats,
                prev_stats: Some(prev_stats),
            });

        let buffer = extract_buffer(reporter);
        let lines: Vec<_> = buffer.lines().collect();
        assert_eq!(lines.len(), 2, "{buffer}");
        assert!(lines[0].starts_with("Benchmarking test @"), "{buffer}");
        assert!(lines[0].contains("printer.rs"), "{buffer}");
        assert_eq!(
            lines[1].trim(),
            "Instructions:             120             +20 (+20.00%)"
        );
    }

    #[test]
    fn reporting_full_stats() {
        let mut reporter = mock_reporter();
        let stats = CachegrindStats::Full(mock_stats());
        reporter
            .new_benchmark(&BenchmarkId::from("test"))
            .ok(&BenchmarkOutput {
                stats,
                prev_stats: None,
            });

        let buffer = extract_buffer(reporter);
        let lines: Vec<_> = buffer.lines().collect();
        assert_eq!(lines.len(), 6, "{buffer}");
        assert!(lines[0].starts_with("Benchmarking test @"), "{buffer}");
        assert!(lines[0].contains("printer.rs"), "{buffer}");
        assert_eq!(lines[1].trim(), "Instructions:             100");
        assert_eq!(lines[2].trim(), "L1 hits     :             250");
        assert_eq!(lines[3].trim(), "L2/L3 hits  :              80");
        assert_eq!(lines[4].trim(), "RAM accesses:              20");
        assert_eq!(lines[5].trim(), "Est. cycles :            1350");
    }

    #[test]
    fn reporting_full_stats_with_diff() {
        let mut reporter = mock_reporter();
        let stats = CachegrindStats::Full(mock_stats());
        let mut prev_stats = mock_stats();
        prev_stats.instructions.total += 10;
        prev_stats.data_reads.l1_misses = 20;
        reporter
            .new_benchmark(&BenchmarkId::from("test"))
            .ok(&BenchmarkOutput {
                stats,
                prev_stats: Some(CachegrindStats::Full(prev_stats)),
            });

        let buffer = extract_buffer(reporter);
        let lines: Vec<_> = buffer.lines().collect();
        assert_eq!(lines.len(), 6, "{buffer}");
        assert!(lines[0].starts_with("Benchmarking test @"), "{buffer}");
        assert!(lines[0].contains("printer.rs"), "{buffer}");
        assert_eq!(
            lines[1].trim(),
            "Instructions:             100             -10 (-9.09%)"
        );
        assert_eq!(
            lines[2].trim(),
            "L1 hits     :             250             -30 (-10.71%)"
        );
        assert_eq!(
            lines[3].trim(),
            "L2/L3 hits  :              80             +20 (+33.33%)"
        );
        assert_eq!(
            lines[4].trim(),
            "RAM accesses:              20     (no change)"
        );
        assert_eq!(
            lines[5].trim(),
            "Est. cycles :            1350             +70 (+5.47%)"
        );
    }
}
