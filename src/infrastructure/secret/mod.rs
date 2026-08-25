//! Infrastructure adapter for secrets management (ADR-018).

pub mod pluggable_secret_store;

pub use pluggable_secret_store::PluggableSecretStore;
