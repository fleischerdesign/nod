//! Application use cases: policy-level operations composed over the ports
//! (ADR-001). These drive the pipeline; they never import Infrastructure.

pub mod adopt;
pub mod audit_log;
pub mod bootstrap_host;
pub mod check_secrets;
pub mod collect_garbage;
pub mod copy_closure;
pub mod deploy_fleet;
pub mod detect_drift;
pub mod eval_fleet;
pub mod exec_fleet;
pub mod export_inventory;
pub mod generate_iso;
pub mod generate_plan;
pub mod health_check;
pub mod inspect_info;
pub mod inspect_metadata;
pub mod list_generations;
pub mod list_inputs;
pub mod optimize_store;
pub mod provider_schedule;
pub mod push_cache;
pub mod reboot_fleet;
pub mod reconcile;
pub mod rekey_secrets;
pub mod render_graph;
pub mod rollback;
pub mod scaffold_flake;
pub mod self_check;
pub mod sync_daemon;
pub mod sync_inspect;
pub mod sync_reconciler;
pub mod sync_status;
pub mod update_flake;
pub mod verify_fleet;
pub mod watch_flake;
