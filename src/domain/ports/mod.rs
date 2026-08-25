//! Domain ports (interfaces) the outside world must satisfy.
//!
//! Domain Core depends on these port *shapes* only; adapters in the
//! Infrastructure layer implement them (ADR-001).

pub mod audit_store;
pub mod config_store;
pub mod deployer;
pub mod evaluator;
pub mod flake;
pub mod health_checker;
pub mod store;
