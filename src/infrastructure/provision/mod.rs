//! Infrastructure adapter for Day-0 provisioning and image generation (ADR-021).

pub mod nixos_anywhere_provisioner;

pub use nixos_anywhere_provisioner::NixosAnywhereProvisioner;
