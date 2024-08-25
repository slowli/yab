//! Reporter implementation printing output to `stderr` in human-readable form.

use std::{
    any::Any,
    cmp::Ordering,
    fmt, io, ops,
    sync::{Arc, Mutex},
    time::Instant,
};

use anes::{
    Attribute, Color, ResetAttributes, SetAttribute, SetBackgroundColor, SetForegroundColor,
};

use super::{BenchmarkOutput, Reporter};
use crate::{
    cachegrind::{AccessSummary, CachegrindStats},
    BenchmarkId, FullCachegrindStats,
};

/// Full width of the label column.
const LABEL_WIDTH: usize = 15;
/// Full width of the number column.
const NUMBER_WIDTH: usize = 16;
/// Width of the diff column (not including percentages).
const DIFF_WIDTH: usize = 12;

#[derive(Debug, Clone, Copy)]
enum Checkmark {
    InProgress,
    Pass,
    Fail,
}

#[derive(Debug)]
struct Styled<'a, W: io::Write>(&'a mut LinePrinter<W>);

impl<W: io::Write> ops::Deref for Styled<'_, W> {
    type Target = LinePrinter<W>;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<W: io::Write> ops::DerefMut for Styled<'_, W> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0
    }
}

impl<W: io::Write> Drop for Styled<'_, W> {
    fn drop(&mut self) {
        if self.0.style_nesting > 0 {
            self.0.style_nesting -= 1;
            if self.0.style_nesting == 0 {
                self.0.print(format_args!("{ResetAttributes}"));
            }
        }
    }
}

#[derive(Debug)]
struct LinePrinter<W> {
    inner: W,
    styling: bool,
    style_nesting: usize,
}

impl<W: io::Write> LinePrinter<W> {
    fn borrow(&mut self) -> Styled<'_, W> {
        if self.styling {
            self.style_nesting += 1;
        }
        Styled(self)
    }

    fn print(&mut self, args: fmt::Arguments<'_>) {
        self.inner
            .write_fmt(args)
            .expect("I/O error writing to stderr");
    }

    fn print_str(&mut self, s: &str) {
        self.inner
            .write_all(s.as_bytes())
            .expect("I/O error writing to stderr");
    }

    fn fg(&mut self, color: Color) -> Styled<'_, W> {
        if self.styling {
            self.print(format_args!("{}", SetForegroundColor(color)));
        }
        self.borrow()
    }

    fn bg(&mut self, color: Color) -> Styled<'_, W> {
        if self.styling {
            self.print(format_args!("{}", SetBackgroundColor(color)));
        }
        self.borrow()
    }

    fn bold(&mut self) -> Styled<'_, W> {
        if self.styling {
            self.print(format_args!("{}", SetAttribute(Attribute::Bold)));
        }
        self.borrow()
    }

    fn dimmed(&mut self) -> Styled<'_, W> {
        if self.styling {
            self.print(format_args!("{}", SetAttribute(Attribute::Faint)));
        }
        self.borrow()
    }

    fn print_checkbox(&mut self, mark: Checkmark) {
        self.print_str("[");
        match mark {
            Checkmark::InProgress => self.fg(Color::Cyan).print_str("*"),
            Checkmark::Pass => self.bold().fg(Color::Green).print_str("√"),
            Checkmark::Fail => self.bold().fg(Color::Red).print_str("x"),
        }
        self.print_str("] ");
    }

    fn print_debug(&mut self, args: fmt::Arguments<'_>) {
        self.bold()
            .bg(Color::DarkMagenta)
            .fg(Color::White)
            .print_str("DEBUG:");
        self.print(format_args!(" {args}\n"));
    }

    fn print_warning(&mut self, id: &BenchmarkId, args: fmt::Arguments<'_>) {
        self.bold()
            .bg(Color::Yellow)
            .fg(Color::White)
            .print_str(" WARN:");
        self.print_str(" ");
        self.print_id(id, true);
        self.print(format_args!(": {args}\n"));
    }

    fn print_error(&mut self, id: Option<&BenchmarkId>, args: fmt::Arguments<'_>) {
        self.bold()
            .bg(Color::Red)
            .fg(Color::White)
            .print_str("ERROR:");
        if let Some(id) = id {
            self.print_str(" ");
            self.print_id(id, true);
            self.print_str(":");
        }
        self.print(format_args!(" {args}\n"));
    }

    fn print_id(&mut self, id: &BenchmarkId, print_location: bool) {
        let BenchmarkId {
            name,
            args,
            location,
        } = id;

        self.print(format_args!("{name}"));
        if let Some(args) = args {
            self.print(format_args!("/{args}"));
        }
        if print_location {
            self.dimmed()
                .print(format_args!(" @ {}:{}", location.file(), location.line()));
        }
    }

    #[allow(clippy::cast_precision_loss, clippy::cast_possible_wrap)] // fine for reporting
    fn print_diff(&mut self, new: u64, old: u64) {
        match new.cmp(&old) {
            Ordering::Less => {
                self.fg(Color::Green).print(format_args!(
                    " {:>+DIFF_WIDTH$} ({:+.2}%)",
                    new as i64 - old as i64,
                    (old - new) as f32 * -100.0 / old as f32
                ));
            }
            Ordering::Greater => {
                self.fg(Color::Red).print(format_args!(
                    " {:>+DIFF_WIDTH$} ({:+.2}%)",
                    new - old,
                    (new - old) as f32 * 100.0 / old as f32
                ));
            }
            Ordering::Equal => { /* don't print anything */ }
        }
    }

    fn print_row(&mut self, label: &str, last: bool, new: u64, old: Option<u64>) {
        const ROW_LABEL_WIDTH: usize = LABEL_WIDTH - 2;

        let line = if last { '└' } else { '├' };
        self.print(format_args!(
            "{line} {label:<ROW_LABEL_WIDTH$} {new:>NUMBER_WIDTH$}"
        ));
        if let Some(old) = old {
            self.print_diff(new, old);
        }
        self.print_str("\n");
    }

    fn print_detail_row(&mut self, label: &str, last: bool, new: u64, old: Option<u64>) {
        const DETAIL_LABEL_WIDTH: usize = LABEL_WIDTH - 4;

        let line = if last { '└' } else { '├' };
        self.print(format_args!(
            "│ {line} {label:<DETAIL_LABEL_WIDTH$} {new:>NUMBER_WIDTH$}"
        ));
        if let Some(old) = old {
            self.print_diff(new, old);
        }
        self.print_str("\n");
    }

    fn print_details(&mut self, new: AccessDetails, old: Option<AccessDetails>) {
        let old_instructions = old.map(|old| old.instructions);
        let print_instr = new.instructions > 0 || old_instructions > Some(0);
        let old_data_reads = old.map(|old| old.data_reads);
        let print_reads = new.data_reads > 0 || old_data_reads > Some(0);
        let old_data_writes = old.map(|old| old.data_writes);
        let print_writes = new.data_writes > 0 || old_data_writes > Some(0);

        if print_instr {
            self.print_detail_row(
                "Instr.",
                !print_reads && !print_writes,
                new.instructions,
                old_instructions,
            );
        }
        if print_reads {
            self.print_detail_row("Data reads", !print_writes, new.data_reads, old_data_reads);
        }
        if print_writes {
            self.print_detail_row("Data writes", true, new.data_writes, old_data_writes);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Verbosity {
    Quiet,
    Normal,
    Verbose,
}

#[derive(Debug)]
pub(crate) struct PrintingReporter<W = io::Stderr> {
    verbosity: Verbosity,
    line_printer: Arc<Mutex<LinePrinter<W>>>,
}

impl<W> Clone for PrintingReporter<W> {
    fn clone(&self) -> Self {
        Self {
            verbosity: self.verbosity,
            line_printer: self.line_printer.clone(),
        }
    }
}

impl PrintingReporter {
    pub(crate) fn new(styling: bool, verbosity: Verbosity) -> Self {
        let line_printer = LinePrinter {
            inner: io::stderr(),
            styling,
            style_nesting: 0,
        };
        Self {
            verbosity,
            line_printer: Arc::new(Mutex::new(line_printer)),
        }
    }

    pub fn report_list_item(id: &BenchmarkId) {
        println!("{id}: benchmark");
    }
}

impl<W: io::Write> PrintingReporter<W> {
    fn lock_printer(&self) -> impl ops::DerefMut<Target = LinePrinter<W>> + '_ {
        self.line_printer.lock().expect("line printer is poisoned")
    }

    pub(crate) fn report_debug(&self, args: fmt::Arguments<'_>) {
        if self.verbosity < Verbosity::Verbose {
            return;
        }
        self.lock_printer().print_debug(args);
    }

    pub(crate) fn report_error(&self, id: Option<&BenchmarkId>, err: &dyn fmt::Display) {
        self.lock_printer().print_error(id, format_args!("{err}"));
    }

    fn report_warning(&self, id: &BenchmarkId, err: &dyn fmt::Display) {
        self.lock_printer().print_warning(id, format_args!("{err}"));
    }
}

#[derive(Debug)]
pub(crate) struct TestReporter<W> {
    parent: PrintingReporter<W>,
    test_id: BenchmarkId,
    started_at: Instant,
}

impl<W: io::Write> super::TestReporter for TestReporter<W> {
    fn ok(self: Box<Self>) {
        let mut printer = self.parent.lock_printer();
        printer.print_checkbox(Checkmark::Pass);
        printer.print_id(&self.test_id, self.parent.verbosity >= Verbosity::Verbose);
        let latency = self.started_at.elapsed();
        printer.print(format_args!(" ({latency:?})\n"));
    }

    fn fail(self: Box<Self>, _: &dyn Any) {
        let mut printer = self.parent.lock_printer();
        printer.print_checkbox(Checkmark::Fail);
        printer.print_id(&self.test_id, self.parent.verbosity >= Verbosity::Verbose);
        printer.print_str(": ");
        printer.bold().fg(Color::Red).print_str("FAILED");
        printer.print_str("\n");
    }
}

#[derive(Debug)]
struct BenchmarkReporter<W> {
    parent: PrintingReporter<W>,
    bench_id: BenchmarkId,
    started_at: Option<Instant>,
}

impl<W: io::Write> BenchmarkReporter<W> {
    fn full_diff(
        &self,
        printer: &mut LinePrinter<W>,
        stats: FullCachegrindStats,
        old_stats: Option<FullCachegrindStats>,
    ) {
        let parent = &self.parent;
        let summary = AccessSummary::from(stats);
        let old_summary = old_stats.map(AccessSummary::from);

        printer.print_row(
            "Instructions",
            false,
            summary.instructions,
            old_summary.map(|old| old.instructions),
        );

        if parent.verbosity >= Verbosity::Normal {
            printer.print_row(
                "L1 hits",
                false,
                summary.l1_hits,
                old_summary.map(|old| old.l1_hits),
            );
            if parent.verbosity >= Verbosity::Verbose {
                printer.print_details(
                    stats.l1_hits(),
                    old_stats.as_ref().map(FullCachegrindStats::l1_hits),
                );
            }

            printer.print_row(
                "L2/L3 hits",
                false,
                summary.l3_hits,
                old_summary.map(|old| old.l3_hits),
            );
            if parent.verbosity >= Verbosity::Verbose {
                printer.print_details(
                    stats.l3_hits(),
                    old_stats.as_ref().map(FullCachegrindStats::l3_hits),
                );
            }

            printer.print_row(
                "RAM accesses",
                false,
                summary.ram_accesses,
                old_summary.map(|old| old.ram_accesses),
            );
            if parent.verbosity >= Verbosity::Verbose {
                printer.print_details(
                    stats.ram(),
                    old_stats.as_ref().map(FullCachegrindStats::ram),
                );
            }
        }

        printer.print_row(
            "Est. cycles",
            true,
            summary.estimated_cycles(),
            old_summary.map(|old| old.estimated_cycles()),
        );
    }
}

impl<W: io::Write + fmt::Debug + Send> super::BenchmarkReporter for BenchmarkReporter<W> {
    fn start_execution(&mut self) {
        self.started_at = Some(Instant::now());
    }

    fn baseline_computed(&mut self, stats: &CachegrindStats) {
        if self.parent.verbosity < Verbosity::Verbose {
            return;
        }

        let mut printer = self.parent.lock_printer();
        printer.print_checkbox(Checkmark::InProgress);
        printer.print_id(&self.bench_id, true);
        let instr = stats.total_instructions();
        printer.print(format_args!(": captured baseline ({instr} instructions)\n"));
    }

    fn ok(self: Box<Self>, output: &BenchmarkOutput) {
        let BenchmarkOutput { stats, prev_stats } = output;

        let mut printer = self.parent.lock_printer();
        printer.print_checkbox(Checkmark::Pass);
        printer.print_id(&self.bench_id, self.parent.verbosity >= Verbosity::Verbose);
        if let Some(started_at) = self.started_at {
            let latency = started_at.elapsed();
            printer.dimmed().print(format_args!(" ({latency:?})"));
        }
        printer.print_str("\n");

        let (stats, prev_stats) = match (*stats, *prev_stats) {
            (CachegrindStats::Simple { instructions }, _) => {
                let old_instructions = prev_stats.as_ref().map(CachegrindStats::total_instructions);
                printer.print_row("Instructions", true, instructions, old_instructions);
                return;
            }
            (_, Some(CachegrindStats::Simple { instructions: old })) => {
                printer.print_row("Instructions", true, stats.total_instructions(), Some(old));
                return;
            }
            (CachegrindStats::Full(stats), None) => (stats, None),
            (CachegrindStats::Full(stats), Some(CachegrindStats::Full(old_stats))) => {
                (stats, Some(old_stats))
            }
        };

        self.full_diff(&mut printer, stats, prev_stats);
    }

    fn warning(&mut self, warning: &dyn fmt::Display) {
        self.parent.report_warning(&self.bench_id, warning);
    }

    fn error(self: Box<Self>, error: &dyn fmt::Display) {
        self.parent.report_error(Some(&self.bench_id), error);
    }
}

impl<W> Reporter for PrintingReporter<W>
where
    W: io::Write + fmt::Debug + Send + 'static,
{
    fn error(&mut self, error: &dyn fmt::Display) {
        self.report_error(None, error);
    }

    fn new_test(&mut self, id: &BenchmarkId) -> Box<dyn super::TestReporter> {
        Box::new(TestReporter {
            parent: self.clone(),
            test_id: id.clone(),
            started_at: Instant::now(),
        })
    }

    fn new_benchmark(&mut self, id: &BenchmarkId) -> Box<dyn super::BenchmarkReporter> {
        if self.verbosity >= Verbosity::Verbose {
            let mut printer = self.lock_printer();
            printer.print_checkbox(Checkmark::InProgress);
            printer.print_id(id, true);
            printer.print(format_args!(": started\n"));
        }

        Box::new(BenchmarkReporter {
            parent: self.clone(),
            bench_id: id.clone(),
            started_at: None,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct AccessDetails {
    instructions: u64,
    data_reads: u64,
    data_writes: u64,
}

impl FullCachegrindStats {
    fn l1_hits(&self) -> AccessDetails {
        AccessDetails {
            instructions: self.instructions.l1_hits(),
            data_reads: self.data_reads.l1_hits(),
            data_writes: self.data_writes.l1_hits(),
        }
    }

    fn l3_hits(&self) -> AccessDetails {
        AccessDetails {
            instructions: self.instructions.l3_hits(),
            data_reads: self.data_reads.l3_hits(),
            data_writes: self.data_writes.l3_hits(),
        }
    }

    fn ram(&self) -> AccessDetails {
        AccessDetails {
            instructions: self.instructions.l3_misses,
            data_reads: self.data_reads.l3_misses,
            data_writes: self.data_writes.l3_misses,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cachegrind::{CachegrindDataPoint, FullCachegrindStats};

    fn mock_reporter(verbosity: Verbosity) -> PrintingReporter<Vec<u8>> {
        let line_printer = LinePrinter {
            inner: vec![],
            styling: false,
            style_nesting: 0,
        };
        PrintingReporter {
            verbosity,
            line_printer: Arc::new(Mutex::new(line_printer)),
        }
    }

    fn extract_buffer(reporter: PrintingReporter<Vec<u8>>) -> String {
        let buffer = Arc::into_inner(reporter.line_printer).unwrap();
        let buffer = buffer.into_inner().unwrap().inner;
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
        let mut reporter = mock_reporter(Verbosity::Normal);
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
        assert!(lines[0].starts_with("[√] test ("), "{buffer}");
        assert!(!lines[0].contains("printer.rs"), "{buffer}");
        assert_eq!(lines[1], "└ Instructions               123");
    }

    #[test]
    fn reporting_basic_stats_with_diff() {
        let mut reporter = mock_reporter(Verbosity::Normal);
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
        assert_eq!(lines[0], "[√] test");
        assert_eq!(
            lines[1],
            "└ Instructions               120          +20 (+20.00%)"
        );
    }

    #[test]
    fn reporting_full_stats() {
        let mut reporter = mock_reporter(Verbosity::Normal);
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
        assert_eq!(lines[0], "[√] test");
        assert_eq!(lines[1], "├ Instructions               100");
        assert_eq!(lines[2], "├ L1 hits                    250");
        assert_eq!(lines[3], "├ L2/L3 hits                  80");
        assert_eq!(lines[4], "├ RAM accesses                20");
        assert_eq!(lines[5], "└ Est. cycles               1350");
    }

    #[test]
    fn reporting_full_stats_verbosely() {
        let mut reporter = mock_reporter(Verbosity::Verbose);
        let stats = CachegrindStats::Full(mock_stats());
        reporter
            .new_benchmark(&BenchmarkId::from("test"))
            .ok(&BenchmarkOutput {
                stats,
                prev_stats: None,
            });

        let buffer = extract_buffer(reporter);
        let lines: Vec<_> = buffer.lines().collect();
        assert!(lines.len() > 10, "{buffer}");
        assert!(lines[0].starts_with("[*] test @"), "{buffer}");
        assert!(lines[0].contains("printer.rs"));
        assert!(lines[1].starts_with("[√] test @"), "{buffer}");
        assert_eq!(lines[2], "├ Instructions               100");
        assert_eq!(lines[3], "├ L1 hits                    250");
        assert_eq!(lines[4], "│ ├ Instr.                    80");
        assert_eq!(lines[5], "│ ├ Data reads               160");
        assert_eq!(lines[6], "│ └ Data writes               10");

        let ram_idx = lines
            .iter()
            .position(|&line| line == "├ RAM accesses                20")
            .unwrap();
        assert_eq!(lines[ram_idx + 1], "│ ├ Instr.                    10");
        assert_eq!(lines[ram_idx + 2], "│ └ Data reads                10");
        assert_eq!(*lines.last().unwrap(), "└ Est. cycles               1350");
    }

    #[test]
    fn reporting_full_stats_with_diff() {
        let mut reporter = mock_reporter(Verbosity::Normal);
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
        assert_eq!(lines[0], "[√] test");
        assert_eq!(
            lines[1],
            "├ Instructions               100          -10 (-9.09%)"
        );
        assert_eq!(
            lines[2],
            "├ L1 hits                    250          -30 (-10.71%)"
        );
        assert_eq!(
            lines[3],
            "├ L2/L3 hits                  80          +20 (+33.33%)"
        );
        assert_eq!(lines[4], "├ RAM accesses                20");
        assert_eq!(
            lines[5],
            "└ Est. cycles               1350          +70 (+5.47%)"
        );
    }
}
