//! `nod reconcile` command: one provider, one cycle (ADR-030).

use std::path::Path;

use crate::application::use_cases::reconcile::{self, ReconcileOptions};
use crate::domain::errors::NodError;

/// Runs one reconcile cycle and renders the report.
pub async fn execute(
    provider: &Path,
    settings_path: Option<&Path>,
    desired_path: &Path,
    apply: bool,
    json: bool,
) -> Result<(), NodError> {
    let settings = match settings_path {
        Some(path) => {
            let raw = std::fs::read_to_string(path).map_err(|error| {
                NodError::config(format!("cannot read settings {}: {error}", path.display()))
            })?;
            serde_json::from_str(&raw).map_err(|error| {
                NodError::config(format!("cannot parse settings {}: {error}", path.display()))
            })?
        }
        None => serde_json::Value::Object(serde_json::Map::new()),
    };

    let raw = std::fs::read_to_string(desired_path).map_err(|error| {
        NodError::config(format!(
            "cannot read desired state {}: {error}",
            desired_path.display()
        ))
    })?;
    let desired = reconcile::parse_desired(&raw)?;

    let report =
        reconcile::execute(provider, &settings, &desired, ReconcileOptions { apply }).await?;

    if json {
        let text = serde_json::to_string_pretty(&serde_json::json!({
            "provider": report.provider,
            "observed": report.observed,
            "planned": report.planned,
            "approved": report.approved.len(),
            "refused": report.refused,
            "applied": report.applied,
            "verified": report.verified,
        }))
        .map_err(|error| NodError::provider(format!("cannot render report: {error}")))?;
        println!("{text}");
        return Ok(());
    }

    println!("provider  {}", report.provider);
    println!("observed  {}", report.observed);
    println!("planned   {}", report.planned);
    println!("approved  {}", report.approved.len());
    println!("refused   {}", report.refused.len());
    for reason in &report.refused {
        println!("          {reason}");
    }
    println!("applied   {}", report.applied);
    println!("verified  {}", report.verified);

    if !apply {
        println!("\ndry run: pass --apply to execute the approved changes");
    }
    Ok(())
}
