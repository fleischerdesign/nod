//! Domain core: pure, dependency-free entities, value objects and ports.
//!
//! Domain must never import from Application, Infrastructure or
//! Presentation (ADR-001).

pub mod audit;
pub mod cache;
pub mod config;
pub mod errors;
pub mod eval;
pub mod flake;
pub mod generation;
pub mod host;
pub mod info;
pub mod plan;
pub mod ports;
pub mod secret;
pub mod ssh_args;
pub mod topology;
