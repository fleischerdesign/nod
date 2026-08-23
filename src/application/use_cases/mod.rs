//! Application use cases: policy-level operations composed over the ports
//! (ADR-001). These drive the pipeline; they never import Infrastructure.

pub mod audit_log;
pub mod deploy_fleet;
pub mod detect_drift;
pub mod generate_plan;
pub mod health_check;
pub mod rollback;