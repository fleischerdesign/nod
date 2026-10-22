//! Adapter module: reconciler state storage.

pub mod json_sync_state;
pub mod provider_lock;

pub use json_sync_state::{JsonSyncStateStore, SyncLock};
