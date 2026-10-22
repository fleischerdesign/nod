//! Operational configuration with provenance (ADR-031).
//!
//! Two scopes: the fleet declaration says *what exists*; this module says *how
//! the process runs*. Operational values cascade (CLI > env > file > default)
//! and every value remembers where it came from.

use std::path::PathBuf;

use serde::Serialize;

/// Where an effective value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// A command-line flag.
    Cli,
    /// A `NOD_*` environment variable.
    Env,
    /// The `[runtime]` section of the configuration file.
    File,
    /// The built-in default.
    Default,
}

impl Source {
    /// Lower-case name for reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Cli => "cli",
            Source::Env => "env",
            Source::File => "file",
            Source::Default => "default",
        }
    }
}

/// A value together with its source.
#[derive(Debug, Clone, Serialize)]
pub struct Resolved<T> {
    /// The effective value.
    pub value: T,
    /// Where it came from.
    pub source: Source,
}

impl<T> Resolved<T> {
    /// Builds a resolved value.
    pub fn new(value: T, source: Source) -> Self {
        Self { value, source }
    }
}

/// Operational configuration for one process.
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeConfig {
    /// Flake root the process works on.
    pub flake: Resolved<String>,
    /// Configuration file, when one is in use.
    pub config_file: Resolved<Option<PathBuf>>,
    /// Directory holding state, plans and locks.
    pub state_dir: Resolved<PathBuf>,
    /// Log level.
    pub log: Resolved<String>,
    /// Whether machine-readable output is the default.
    pub json: Resolved<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_names_itself_in_lower_case() {
        assert_eq!(Source::Cli.as_str(), "cli");
        assert_eq!(Source::Default.as_str(), "default");
    }
}
