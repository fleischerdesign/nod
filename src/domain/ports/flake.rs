//! Domain port for Flake and Lockfile management (ADR-014).

use async_trait::async_trait;
use std::path::Path;

use crate::domain::errors::NodError;
use crate::domain::flake::{FlakeInputNode, FlakeMetadata, FlakeUpdateReport};

/// SPI port for inspecting and updating Nix flakes and their lockfiles.
#[async_trait]
pub trait FlakePort: Send + Sync {
    /// Loads high-level repository and lockfile metadata.
    async fn load_metadata(&self, flake_path: &Path) -> Result<FlakeMetadata, NodError>;

    /// Loads the resolved input nodes from the flake.
    async fn load_inputs(&self, flake_path: &Path) -> Result<Vec<FlakeInputNode>, NodError>;

    /// Updates specified inputs (or all if empty) and returns the diff report.
    async fn update_inputs(
        &self,
        flake_path: &Path,
        inputs: &[String],
    ) -> Result<FlakeUpdateReport, NodError>;
}
