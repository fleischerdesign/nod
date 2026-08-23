//! Typed configuration value objects for the merged four-tier hierarchy
//! (ADR-004). These are immutable, dependency-free domain values; tiers are
//! resolved once by the config store and exposed in merged form here.
//!
//! Precedence (fixed per option, higher beats lower):
//!   1. CLI overrides  (`CliOverrides`)
//!   2. Local `.nod.toml`  (`[hosts.<name>]` > `[fleet]` > `[defaults]`)
//!   3. Flake metadata (materialized on the `HostEntity`)
//!   4. Compiled-in defaults (`root` / `22`, also on the `HostEntity`)

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Highest-tier, per-run SSH overrides supplied by CLI flags. Applied on top
/// of every resolved profile after the `.nod.toml` and flake tiers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliOverrides {
    /// CLI `--user` override, empty when not supplied.
    pub user: Option<String>,
    /// CLI `--port` override, empty when not supplied.
    pub port: Option<u16>,
    /// CLI `--identity-file` override, empty when not supplied.
    pub identity_file: Option<PathBuf>,
}

/// Merged per-host overrides exposed by the config store: TOML `[hosts.<name>]`
/// values over `[fleet]` over `[defaults]`. `target_host`, `role` and `tags`
/// are host-section-specific.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostOverrides {
    pub target_host: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
    pub proxy_jump: Option<String>,
    pub sudo: Option<bool>,
    pub role: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// Fleet-wide merged defaults: TOML `[fleet]`/`[defaults]` values (persistent
/// policy) with no CLI tier. Ships the connection defaults that every host
/// inherits when it has no specific override.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetDefaults {
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
    pub proxy_jump: Option<String>,
    pub sudo: Option<bool>,
}