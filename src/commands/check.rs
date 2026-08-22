use crate::infrastructure::quality_gate::QualityGate;
use anyhow::Result;
use std::path::Path;

pub async fn execute(flake_path: &Path) -> Result<()> {
    QualityGate::run_all(flake_path).await
}
