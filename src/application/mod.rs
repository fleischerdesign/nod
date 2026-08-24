//! Application layer: use-case policy and the AppContext DI container.
//!
//! Depends on Domain only through ports (ADR-001).

pub mod context;
pub mod pipeline;
pub mod selection;
pub mod spawn;
pub mod use_cases;
