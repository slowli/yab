use std::{
    cmp::Ordering,
    fmt, io,
    io::{IsTerminal, Write},
    time::Instant,
};

use anes::{Attribute, ClearLine, Color, ResetAttributes, SetAttribute, SetForegroundColor};

use crate::cachegrind::{AccessSummary, CachegrindSummary};

#[derive(Debug)]
pub(crate) struct Reporter {
    overwrite: bool,
    styling: bool,
}

impl Default for Reporter {
    fn default() -> Self {
        Self {
            overwrite: io::stderr().is_terminal(),
            styling: io::stderr().is_terminal(),
        }
    }
}

impl Reporter {
    fn print_overwritable(&mut self, message: &str) {
        if self.overwrite {
            eprint!("{message}");
            io::stderr().flush().ok();
        } else {
            eprintln!("{message}");
        }
    }

    fn overwrite_line(&mut self) {
        if self.overwrite {
            eprint!("\r{}", ClearLine::All);
        }
    }

    fn print(&mut self, message: &str) {
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

    pub fn report_test(&mut self, name: &str) -> TestReporter<'_> {
        self.print_overwritable(&format!("Testing {name}:"));
        TestReporter {
            parent: self,
            test_name: name.to_owned(),
            started_at: Instant::now(),
        }
    }

    pub fn report_bench(&mut self, name: &str) -> BenchReporter<'_> {
        self.print_overwritable(&format!("Benchmarking {name}:"));
        BenchReporter {
            parent: self,
            bench_name: name.to_owned(),
            started_at: Some(Instant::now()),
        }
    }

    pub fn report_bench_result(&mut self, name: &str) -> BenchReporter<'_> {
        self.print_overwritable(&format!("Loading benchmark {name}:"));
        BenchReporter {
            parent: self,
            bench_name: name.to_owned(),
            started_at: None,
        }
    }

    pub fn report_list_item(&mut self, name: &str) {
        println!("{name}: benchmark");
    }

    pub fn report_fatal_error(&mut self, err: &dyn fmt::Display) {
        let fatal = self.with_color(self.bold("FATAL:".into()), Color::Red);
        eprintln!("{fatal} {err}");
    }

    pub fn report_warning(&mut self, err: &dyn fmt::Display) {
        let warn = self.with_color(self.bold("WARN:".into()), Color::Yellow);
        eprintln!("{warn} {err}");
    }
}

#[derive(Debug)]
#[must_use = "Test outcome should be reported"]
pub(crate) struct TestReporter<'a> {
    parent: &'a mut Reporter,
    test_name: String,
    started_at: Instant,
}

impl TestReporter<'_> {
    pub fn fail(self) {
        self.parent.overwrite_line();
        let name = &self.test_name;
        self.parent.print(&format!("Testing {name}: FAILED"));
    }

    pub fn ok(self) {
        self.parent.overwrite_line();
        let name = &self.test_name;
        let latency = self.started_at.elapsed();
        self.parent
            .print(&format!("Testing {name}: OK ({latency:?})"));
    }
}

#[derive(Debug)]
#[must_use = "Test outcome should be reported"]
pub(crate) struct BenchReporter<'a> {
    parent: &'a mut Reporter,
    bench_name: String,
    started_at: Option<Instant>,
}

impl BenchReporter<'_> {
    pub fn no_data(self) {
        self.parent.overwrite_line();
        let name = &self.bench_name;
        let no_data = self.parent.bold("no data".into());
        self.parent
            .print(&format!("Benchmarking {name}: {no_data}"));
    }

    pub fn ok(self, summary: CachegrindSummary, old_summary: Option<CachegrindSummary>) {
        self.parent.overwrite_line();
        let name = &self.bench_name;
        let ok = self.parent.bold("OK".into());
        let latency = if let Some(started_at) = self.started_at {
            let latency = started_at.elapsed();
            self.parent
                .with_color(format!(" ({latency:?})"), Color::Gray)
        } else {
            String::new()
        };
        self.parent
            .print(&format!("Benchmarking {name}: {ok}{latency}"));

        let access_summary: AccessSummary = summary.into();
        let old_access_summary = old_summary.map(AccessSummary::from);

        let diff = old_access_summary
            .map(|old| self.report_diff(access_summary.instructions, old.instructions))
            .unwrap_or_default();
        self.parent.print(&format!(
            "  Instructions: {:>15} {diff}",
            access_summary.instructions
        ));

        let diff = old_access_summary
            .map(|old| self.report_diff(access_summary.l1_hits, old.l1_hits))
            .unwrap_or_default();
        self.parent.print(&format!(
            "  L1 hits     : {:>15} {diff}",
            access_summary.l1_hits
        ));

        let diff = old_access_summary
            .map(|old| self.report_diff(access_summary.l3_hits, old.l3_hits))
            .unwrap_or_default();
        self.parent.print(&format!(
            "  L2/L3 hits  : {:>15} {diff}",
            access_summary.l3_hits
        ));

        let diff = old_access_summary
            .map(|old| self.report_diff(access_summary.ram_accesses, old.ram_accesses))
            .unwrap_or_default();
        self.parent.print(&format!(
            "  RAM accesses: {:>15} {diff}",
            access_summary.ram_accesses
        ));

        let diff = old_access_summary
            .map(|old| self.report_diff(access_summary.estimated_cycles(), old.estimated_cycles()))
            .unwrap_or_default();
        self.parent.print(&format!(
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
