//! Benchmark identifiers.

use std::{
    fmt,
    hash::{Hash, Hasher},
    panic::Location,
};

#[derive(Debug, Clone)]
pub struct BenchmarkId {
    pub(crate) name: String,
    pub(crate) location: &'static Location<'static>,
    pub(crate) args: Option<String>, // TODO: is this needed?
}

impl PartialEq for BenchmarkId {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.args == other.args
    }
}

impl PartialEq<&str> for BenchmarkId {
    fn eq(&self, other: &&str) -> bool {
        if let Some(args) = &self.args {
            self.name.len() + 1 + args.len() == other.len()
                && other.starts_with(&self.name)
                && other.ends_with(args)
                && other.as_bytes()[self.name.len()] == b'/'
        } else {
            self.name == *other
        }
    }
}

impl Eq for BenchmarkId {}

impl Hash for BenchmarkId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.args.hash(state);
    }
}

impl fmt::Display for BenchmarkId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(args) = &self.args {
            write!(formatter, "{}/{args}", self.name)
        } else {
            formatter.write_str(&self.name)
        }
    }
}

impl<S: Into<String>> From<S> for BenchmarkId {
    #[track_caller]
    fn from(name: S) -> Self {
        Self {
            name: name.into(),
            location: Location::caller(),
            args: None,
        }
    }
}

impl BenchmarkId {
    #[track_caller]
    pub fn new(name: impl Into<String>, args: impl fmt::Display) -> Self {
        Self {
            name: name.into(),
            location: Location::caller(),
            args: Some(args.to_string()),
        }
    }
}
