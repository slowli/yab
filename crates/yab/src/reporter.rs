use std::{
    cmp::Ordering,
    fmt, io,
    io::{IsTerminal, Write},
    ops,
    sync::{Arc, Mutex},
    time::Instant,
};

use anes::{Attribute, ClearLine, Color, ResetAttributes, SetAttribute, SetForegroundColor};

use crate::{
    cachegrind::{AccessSummary, CachegrindSummary},
    BenchmarkId,
};

#[derive(Debug, Clone, Default)]
pub(crate) struct Reporter(Arc<Mutex<ReporterInner>>);

#[derive(Debug)]
pub(crate) struct ReporterInner {
    overwrite: bool,
    styling: bool,
    has_overwritable_line: bool,
}

impl Default for ReporterInner {
    fn default() -> Self {
        Self {
            overwrite: false, // io::stderr().is_terminal(),
            styling: io::stderr().is_terminal(),
            has_overwritable_line: false,
        }
    }
}

impl ReporterInner {
    #[allow(dead_code)] // FIXME
    fn print_overwritable(&mut self, message: &str) {
        if self.overwrite {
            eprint!("{message}");
            io::stderr().flush().ok();
            self.has_overwritable_line = true;
        } else {
            eprintln!("{message}");
        }
    }

    #[allow(dead_code)] // FIXME
    fn overwrite_line(&mut self) {
        if self.overwrite && self.has_overwritable_line {
            self.has_overwritable_line = false;
            eprint!("\r{}", ClearLine::All);
        }
    }

    fn println(&mut self, message: &str) {
        if self.overwrite && self.has_overwritable_line {
            eprintln!(); // "finalize" the overwritable line
        }
        eprintln!("{message}");
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
}

impl Reporter {
    fn lock(&self) -> impl ops::DerefMut<Target = ReporterInner> + '_ {
        self.0.lock().unwrap()
    }

    pub fn report_test(&self, id: &BenchmarkId) -> TestReporter<'_> {
        let test_id = self.lock().format_id(id);
        TestReporter {
            parent: self,
            test_id,
            started_at: Instant::now(),
        }
    }

    pub fn report_bench(&self, id: &BenchmarkId) -> BenchReporter<'_> {
        let bench_id = self.lock().format_id(id);
        BenchReporter {
            parent: self,
            bench_id,
            started_at: Some(Instant::now()),
        }
    }

    pub fn report_bench_result(&self, id: &BenchmarkId) -> BenchReporter<'_> {
        let bench_id = self.lock().format_id(id);
        BenchReporter {
            parent: self,
            bench_id,
            started_at: None,
        }
    }

    pub fn report_list_item(&self, id: &BenchmarkId) {
        println!("{id}: benchmark");
    }

    pub fn report_fatal_error(&self, err: &dyn fmt::Display) {
        let mut inner = self.lock();
        let fatal = inner.with_color(inner.bold("FATAL:".into()), Color::Red);
        inner.println(&format!("{fatal} {err}"));
    }

    pub fn report_warning(&self, err: &dyn fmt::Display) {
        let mut inner = self.lock();
        let warn = inner.with_color(inner.bold("WARN:".into()), Color::Yellow);
        inner.println(&format!("{warn} {err}"));
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
        let mut inner = self.parent.lock();
        let no_data = inner.bold("no data".into());
        inner.println(&format!("Benchmarking {id}: {no_data}"));
    }

    pub fn baseline(&self, summary: &CachegrindSummary) {
        let id = &self.bench_id;
        let instr = summary.instructions.total;
        self.parent.lock().println(&format!(
            "Benchmarking {id}: captured baseline ({instr} instructions)"
        ));
    }

    pub fn ok(self, summary: CachegrindSummary, old_summary: Option<CachegrindSummary>) {
        let mut inner = self.parent.lock();
        let id = &self.bench_id;
        let ok = inner.bold("OK".into());
        let latency = if let Some(started_at) = self.started_at {
            let latency = started_at.elapsed();
            inner.dimmed(format!(" ({latency:?})"))
        } else {
            String::new()
        };
        inner.println(&format!("Benchmarking {id}: {ok}{latency}"));

        let access_summary: AccessSummary = summary.into();
        let old_access_summary = old_summary.map(AccessSummary::from);

        let diff = old_access_summary
            .map(|old| inner.report_diff(access_summary.instructions, old.instructions))
            .unwrap_or_default();
        inner.println(&format!(
            "  Instructions: {:>15} {diff}",
            access_summary.instructions
        ));

        let diff = old_access_summary
            .map(|old| inner.report_diff(access_summary.l1_hits, old.l1_hits))
            .unwrap_or_default();
        inner.println(&format!(
            "  L1 hits     : {:>15} {diff}",
            access_summary.l1_hits
        ));

        let diff = old_access_summary
            .map(|old| inner.report_diff(access_summary.l3_hits, old.l3_hits))
            .unwrap_or_default();
        inner.println(&format!(
            "  L2/L3 hits  : {:>15} {diff}",
            access_summary.l3_hits
        ));

        let diff = old_access_summary
            .map(|old| inner.report_diff(access_summary.ram_accesses, old.ram_accesses))
            .unwrap_or_default();
        inner.println(&format!(
            "  RAM accesses: {:>15} {diff}",
            access_summary.ram_accesses
        ));

        let diff = old_access_summary
            .map(|old| inner.report_diff(access_summary.estimated_cycles(), old.estimated_cycles()))
            .unwrap_or_default();
        inner.println(&format!(
            "  Est. cycles : {:>15} {diff}",
            access_summary.estimated_cycles()
        ));
    }
}
