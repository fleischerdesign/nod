//! Domain core: pure, dependency-free entities, value objects and ports.
//!
//! Domain must never import from Application, Infrastructure or
//! Presentation (ADR-001).

pub mod adoption;
pub mod audit;
pub mod cache;
pub mod config;
pub mod desired;
pub mod errors;
pub mod eval;
pub mod flake;
pub mod generation;
pub mod health;
pub mod host;
pub mod info;
pub mod ownership;
pub mod plan;
pub mod policy;
pub mod ports;
pub mod provider;
pub mod provision;
pub mod runtime_config;
pub mod secret;
pub mod ssh_args;
pub mod sync;
pub mod topology;
pub mod watch;
