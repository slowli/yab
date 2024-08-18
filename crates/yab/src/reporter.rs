use std::{
    cmp::Ordering,
    fmt, io,
    io::IsTerminal,
    ops::DerefMut,
    sync::{Arc, Mutex, PoisonError},
    time::Instant,
};

use anes::{Attribute, Color, ResetAttributes, SetAttribute, SetForegroundColor};

use crate::{
    cachegrind::{AccessSummary, CachegrindStats},
    BenchmarkId,
};

#[derive(Debug, Default)]
struct LinePrinter;

impl LinePrinter {
    #[allow(clippy::unused_self)] // intentional design choice
    fn println(&mut self, line: &str) {
        eprintln!("{line}");
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Reporter {
    styling: bool,
    line_printer: Arc<Mutex<LinePrinter>>,
}

impl Default for Reporter {
    fn default() -> Self {
        Self {
            styling: io::stderr().is_terminal(),
            line_printer: Arc::default(),
        }
    }
}

impl Reporter {
    fn lock(&self) -> impl DerefMut<Target = LinePrinter> + '_ {
        // since printer doesn't have state, it cannot be poisoned
        self.line_printer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
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

    pub fn report_test(&self, id: &BenchmarkId) -> TestReporter<'_> {
        let test_id = self.format_id(id);
        TestReporter {
            parent: self,
            test_id,
            started_at: Instant::now(),
        }
    }

    pub fn report_bench(&self, id: &BenchmarkId) -> BenchReporter<'_> {
        let bench_id = self.format_id(id);
        BenchReporter {
            parent: self,
            bench_id,
            started_at: Some(Instant::now()),
        }
    }

    pub fn report_bench_result(&self, id: &BenchmarkId) -> BenchReporter<'_> {
        let bench_id = self.format_id(id);
        BenchReporter {
            parent: self,
            bench_id,
            started_at: None,
        }
    }

    pub fn report_list_item(id: &BenchmarkId) {
        println!("{id}: benchmark");
    }

    pub fn report_fatal_error(&self, err: &dyn fmt::Display) {
        let fatal = self.with_color(self.bold("FATAL:".into()), Color::Red);
        self.lock().println(&format!("{fatal} {err}"));
    }

    pub fn report_warning(&self, err: &dyn fmt::Display) {
        let warn = self.with_color(self.bold("WARN:".into()), Color::Yellow);
        self.lock().println(&format!("{warn} {err}"));
    }
}

#[derive(Debug)]
#[must_use = "Test outcome should be reported"]
pub(crate) struct TestReporter<'a> {
    parent: &'a Reporter,
    test_id: String,
    started_at: Instant,
}

impl TestReporter<'_> {
    pub fn fail(self) {
        let id = &self.test_id;
        self.parent.lock().println(&format!("Testing {id}: FAILED"));
    }

    pub fn ok(self) {
        let id = &self.test_id;
        let latency = self.started_at.elapsed();
        self.parent
            .lock()
            .println(&format!("Testing {id}: OK ({latency:?})"));
    }
}

#[derive(Debug)]
#[must_use = "Test outcome should be reported"]
pub(crate) struct BenchReporter<'a> {
    parent: &'a Reporter,
    bench_id: String,
    started_at: Option<Instant>,
}

impl BenchReporter<'_> {
    pub fn no_data(self) {
        let id = &self.bench_id;
        let no_data = self.parent.bold("no data".into());
        self.parent
            .lock()
            .println(&format!("Benchmarking {id}: {no_data}"));
    }

    pub fn baseline(&self, summary: &CachegrindStats) {
        let id = &self.bench_id;
        let instr = summary.total_instructions();
        self.parent.lock().println(&format!(
            "Benchmarking {id}: captured baseline ({instr} instructions)"
        ));
    }

    pub fn ok(self, stats: CachegrindStats, old_stats: Option<CachegrindStats>) {
        let mut printer = self.parent.lock();
        let id = &self.bench_id;
        let ok = self.parent.bold("OK".into());
        let latency = if let Some(started_at) = self.started_at {
            let latency = started_at.elapsed();
            self.parent.dimmed(format!(" ({latency:?})"))
        } else {
            String::new()
        };
        printer.println(&format!("Benchmarking {id}: {ok}{latency}"));

        let (stats, old_stats) = match (stats, old_stats) {
            (CachegrindStats::Simple { instructions }, _) => {
                let old_instructions = old_stats.as_ref().map(CachegrindStats::total_instructions);
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
            old_stats.map(AccessSummary::from),
        );
    }

    fn instruction_diff(&self, printer: &mut LinePrinter, new: u64, old: Option<u64>) {
        let diff = old
            .map(|old| self.parent.report_diff(new, old))
            .unwrap_or_default();
        printer.println(&format!("  Instructions: {new:>15} {diff}"));
    }

    fn full_diff(
        &self,
        printer: &mut LinePrinter,
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
