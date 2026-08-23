//! Domain core: pure, dependency-free entities, value objects and ports.
//!
//! Domain must never import from Application, Infrastructure or
//! Presentation (ADR-001).

pub mod config;
pub mod errors;
pub mod history;
pub mod host;
pub mod plan;
pub mod ports;