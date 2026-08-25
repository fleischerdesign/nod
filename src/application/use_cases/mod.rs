//! Application use cases: policy-level operations composed over the ports
//! (ADR-001). These drive the pipeline; they never import Infrastructure.

pub mod audit_log;
pub mod check_secrets;
pub mod collect_garbage;
pub mod copy_closure;
pub mod deploy_fleet;
pub mod detect_drift;
pub mod eval_fleet;
pub mod exec_fleet;
pub mod generate_plan;
pub mod health_check;
pub mod inspect_info;
pub mod inspect_metadata;
pub mod list_generations;
pub mod list_inputs;
pub mod reboot_fleet;
pub mod rekey_secrets;
pub mod rollback;
pub mod update_flake;
