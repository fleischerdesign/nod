use crate::domain::errors::NodError;
use crate::infrastructure::quality_gate::QualityGate;
use std::path::Path;

/// Runs the strict repository quality gates (nixfmt + deadnix + statix).
///
/// Returns the typed [`NodError`] rather than a flat `anyhow::Result` so the
/// presentation layer (ADR-002) can branch on the failure class; the caller
/// in `main` still reports it through its anyhow error sink.
pub async fn execute(flake_path: &Path) -> Result<(), NodError> {
    QualityGate::run_all(flake_path).await?;
    Ok(())
}
