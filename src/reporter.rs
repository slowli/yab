use std::{
    cmp::Ordering,
    fmt, io,
    io::{IsTerminal, Write},
    time::Instant,
};

use anes::{Attribute, ClearLine, Color, ResetAttributes, SetAttribute, SetForegroundColor};

use crate::{
    cachegrind::{AccessSummary, CachegrindSummary},
    BenchmarkId,
};

#[derive(Debug)]
pub(crate) struct Reporter {
    overwrite: bool,
    styling: bool,
    has_overwritable_line: bool,
}

impl Default for Reporter {
    fn default() -> Self {
        Self {
            overwrite: io::stderr().is_terminal(),
            styling: io::stderr().is_terminal(),
            has_overwritable_line: false,
        }
    }
}

impl Reporter {
    fn print_overwritable(&mut self, message: &str) {
        if self.overwrite {
            eprint!("{message}");
            io::stderr().flush().ok();
            self.has_overwritable_line = true;
        } else {
            eprintln!("{message}");
        }
    }

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

    pub fn report_test(&mut self, id: &BenchmarkId) -> TestReporter<'_> {
        let test_id = self.format_id(id);
        self.print_overwritable(&format!("Testing {test_id}:"));
        TestReporter {
            parent: self,
            test_id,
            started_at: Instant::now(),
        }
    }

    pub fn report_bench(&mut self, id: &BenchmarkId) -> BenchReporter<'_> {
        let bench_id = self.format_id(id);
        self.print_overwritable(&format!("Benchmarking {bench_id}:"));
        BenchReporter {
            parent: self,
            bench_id,
            started_at: Some(Instant::now()),
        }
    }

    pub fn report_bench_result(&mut self, id: &BenchmarkId) -> BenchReporter<'_> {
        let bench_id = self.format_id(id);
        self.print_overwritable(&format!("Loading benchmark {bench_id}:"));
        BenchReporter {
            parent: self,
            bench_id,
            started_at: None,
        }
    }

    pub fn report_list_item(&mut self, id: &BenchmarkId) {
        println!("{id}: benchmark");
    }

    pub fn report_fatal_error(&mut self, err: &dyn fmt::Display) {
        let fatal = self.with_color(self.bold("FATAL:".into()), Color::Red);
        self.println(&format!("{fatal} {err}"));
    }

    pub fn report_warning(&mut self, err: &dyn fmt::Display) {
        let warn = self.with_color(self.bold("WARN:".into()), Color::Yellow);
        self.println(&format!("{warn} {err}"));
    }
}

#[derive(Debug)]
#[must_use = "Test outcome should be reported"]
pub(crate) struct TestReporter<'a> {
    parent: &'a mut Reporter,
    test_id: String,
    started_at: Instant,
}

impl TestReporter<'_> {
    pub fn fail(self) {
        self.parent.overwrite_line();
        let id = &self.test_id;
        self.parent.println(&format!("Testing {id}: FAILED"));
    }

    pub fn ok(self) {
        self.parent.overwrite_line();
        let id = &self.test_id;
        let latency = self.started_at.elapsed();
        self.parent
            .println(&format!("Testing {id}: OK ({latency:?})"));
    }
}

#[derive(Debug)]
#[must_use = "Test outcome should be reported"]
pub(crate) struct BenchReporter<'a> {
    parent: &'a mut Reporter,
    bench_id: String,
    started_at: Option<Instant>,
}

impl BenchReporter<'_> {
    pub fn no_data(self) {
        self.parent.overwrite_line();
        let id = &self.bench_id;
        let no_data = self.parent.bold("no data".into());
        self.parent
            .println(&format!("Benchmarking {id}: {no_data}"));
    }

    // Logically, this should consume `self`; it doesn't to satisfy the borrow checker.
    pub fn fatal(&mut self, err: &dyn fmt::Display) {
        self.parent.report_fatal_error(err);
    }

    pub fn calibration(&mut self, summary: &CachegrindSummary, iterations: u64) {
        self.parent.overwrite_line();
        let id = &self.bench_id;
        let instr = summary.instructions.total;
        self.parent.print_overwritable(&format!(
            "Benchmarking {id}: calibrated (~{instr} instructions / iter), \
             will use {iterations} iterations"
        ));
    }

    pub fn baseline(&mut self, summary: &CachegrindSummary) {
        self.parent.overwrite_line();
        let id = &self.bench_id;
        let instr = summary.instructions.total;
        self.parent.print_overwritable(&format!(
            "Benchmarking {id}: captured baseline ({instr} instructions)"
        ));
    }

    pub fn ok(self, summary: CachegrindSummary, old_summary: Option<CachegrindSummary>) {
        self.parent.overwrite_line();
        let id = &self.bench_id;
        let ok = self.parent.bold("OK".into());
        let latency = if let Some(started_at) = self.started_at {
            let latency = started_at.elapsed();
            self.parent.dimmed(format!(" ({latency:?})"))
        } else {
            String::new()
        };
        self.parent
            .println(&format!("Benchmarking {id}: {ok}{latency}"));

        let access_summary: AccessSummary = summary.into();
        let old_access_summary = old_summary.map(AccessSummary::from);

        let diff = old_access_summary
            .map(|old| self.report_diff(access_summary.instructions, old.instructions))
            .unwrap_or_default();
        self.parent.println(&format!(
            "  Instructions: {:>15} {diff}",
            access_summary.instructions
        ));

        let diff = old_access_summary
            .map(|old| self.report_diff(access_summary.l1_hits, old.l1_hits))
            .unwrap_or_default();
        self.parent.println(&format!(
            "  L1 hits     : {:>15} {diff}",
            access_summary.l1_hits
        ));

        let diff = old_access_summary
            .map(|old| self.report_diff(access_summary.l3_hits, old.l3_hits))
            .unwrap_or_default();
        self.parent.println(&format!(
            "  L2/L3 hits  : {:>15} {diff}",
            access_summary.l3_hits
        ));

        let diff = old_access_summary
            .map(|old| self.report_diff(access_summary.ram_accesses, old.ram_accesses))
            .unwrap_or_default();
        self.parent.println(&format!(
            "  RAM accesses: {:>15} {diff}",
            access_summary.ram_accesses
        ));

        let diff = old_access_summary
            .map(|old| self.report_diff(access_summary.estimated_cycles(), old.estimated_cycles()))
            .unwrap_or_default();
        self.parent.println(&format!(
            "  Est. cycles : {:>15} {diff}",
            access_summary.estimated_cycles()
        ));
    }

    fn report_diff(&self, new: u64, old: u64) -> String {
        match new.cmp(&old) {
            Ordering::Less => {
                let diff = format!(
                    "{:>+15} ({:+.2}%)",
                    new as i64 - old as i64,
                    (old - new) as f32 * -100.0 / old as f32
                );
                self.parent.with_color(diff, Color::Green)
            }
            Ordering::Greater => {
                let diff = format!(
                    "{:>+15} ({:+.2}%)",
                    new - old,
                    (new - old) as f32 * 100.0 / old as f32
                );
                self.parent.with_color(diff, Color::Red)
            }
            Ordering::Equal => "    (no change)".to_owned(),
        }
    }
}
