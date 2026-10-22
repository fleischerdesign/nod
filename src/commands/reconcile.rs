//! `nod reconcile` command: one provider, one cycle or one rollback (ADR-030).

use std::path::Path;

use crate::application::use_cases::reconcile::{self, ReconcileOptions};
use crate::domain::config::CliOverrides;
use crate::domain::errors::NodError;
use crate::domain::provider::RollbackPlan;
use crate::domain::sync::now_secs;
use crate::infrastructure::config::toml_config::TomlConfigStore;
use crate::infrastructure::state::provider_lock::ProviderLock;
use crate::infrastructure::storage::provider_plan::ProviderPlanStore;

/// Runs one reconcile cycle or one rollback, and renders the result.
///
/// The provider is resolved from `[providers.<name>]` in the configuration; a
/// `--command` overrides the configured executable, and `--settings` overrides
/// the configured settings. Nothing is discovered by convention.
#[allow(clippy::too_many_arguments)]
pub async fn execute(
    provider: Option<&str>,
    command: Option<&Path>,
    settings_path: Option<&Path>,
    desired_path: Option<&Path>,
    apply: bool,
    rollback: bool,
    state_dir: &Path,
    config: Option<&Path>,
    json: bool,
    flake: &str,
) -> Result<(), NodError> {
    let store = match config {
        Some(path) => TomlConfigStore::from_file(path, CliOverrides::default())?,
        None => TomlConfigStore::new(Path::new(flake), CliOverrides::default())?,
    };

    // The fleet lives for the whole function so the resolved provider can be
    // borrowed instead of cloned.
    let fleet = match provider {
        Some(_) => {
            Some(crate::infrastructure::config::fleet::load(Path::new(flake), &store).await?)
        }
        None => None,
    };
    let configured = match provider {
        Some(name) => Some(
            fleet
                .as_ref()
                .and_then(|fleet| fleet.providers.get(name))
                .ok_or_else(|| {
                    NodError::config(format!(
                        "provider `{name}` is not declared in the fleet declaration"
                    ))
                })?,
        ),
        None => None,
    };

    let command_path = command
        .or_else(|| configured.map(|entry| entry.command.as_path()))
        .ok_or_else(|| {
            NodError::config("pass a provider name declared in the config, or --command <path>")
        })?;

    // One reconciliation per provider at a time, across processes. An ad-hoc
    // run without a name has no namespace to lock and stays unlocked.
    let _lock = match provider {
        Some(name) => Some(ProviderLock::acquire(state_dir, name)?),
        None => None,
    };

    let settings = match settings_path {
        Some(path) => read_json(path)?,
        None => match configured.and_then(|entry| entry.settings.as_ref()) {
            Some(value) => value.clone(),
            None => serde_json::Value::Object(serde_json::Map::new()),
        },
    };

    if rollback {
        return run_rollback(provider, command_path, &settings, state_dir, json).await;
    }

    let desired_path = match desired_path {
        Some(path) => path.to_path_buf(),
        None => configured
            .and_then(|entry| entry.desired.clone())
            .ok_or_else(|| {
                NodError::config("pass --desired or declare `desired` for the provider")
            })?,
    };

    let raw = std::fs::read_to_string(&desired_path).map_err(|error| {
        NodError::config(format!(
            "cannot read desired state {}: {error}",
            desired_path.display()
        ))
    })?;
    let desired = reconcile::parse_desired(&raw)?;

    let report = reconcile::execute(
        command_path,
        &settings,
        &desired,
        ReconcileOptions { apply },
    )
    .await?;

    // Record what would undo this run, so `--rollback` has something to apply.
    if apply && !report.inverse.is_empty() {
        let plans = ProviderPlanStore::new(state_dir);
        plans.save(&RollbackPlan {
            provider: report.provider.clone(),
            applied_at: now_secs(),
            changes: report.inverse.clone(),
        })?;
    }

    if json {
        let text = serde_json::to_string_pretty(&serde_json::json!({
            "provider": report.provider,
            "version": report.version,
            "observed": report.observed,
            "planned": report.planned,
            "approved": report.approved.len(),
            "refused": report.refused,
            "applied": report.applied,
            "verified": report.verified,
            "rollback_recorded": apply && !report.inverse.is_empty(),
        }))
        .map_err(|error| NodError::provider(format!("cannot render report: {error}")))?;
        println!("{text}");
        return Ok(());
    }

    println!("provider  {} {}", report.provider, report.version);
    println!("observed  {}", report.observed);
    println!("planned   {}", report.planned);
    println!("approved  {}", report.approved.len());
    println!("refused   {}", report.refused.len());
    for reason in &report.refused {
        println!("          {reason}");
    }
    println!("applied   {}", report.applied);
    println!("verified  {}", report.verified);
    if apply && !report.inverse.is_empty() {
        println!("rollback  recorded ({} operation(s))", report.inverse.len());
    }

    let outcome = if !report.refused.is_empty() {
        "refused"
    } else if report.applied > 0 {
        "applied"
    } else if report.planned > 0 {
        "planned"
    } else {
        "unchanged"
    };
    record_audit(&report.provider, outcome).await;

    if !apply {
        println!("\ndry run: pass --apply to execute the approved changes");
    }
    Ok(())
}

/// Applies the recorded plan for `provider`, or refuses when the world moved.
async fn run_rollback(
    provider: Option<&str>,
    command: &Path,
    settings: &serde_json::Value,
    state_dir: &Path,
    json: bool,
) -> Result<(), NodError> {
    let name = provider
        .ok_or_else(|| NodError::config("--rollback needs a provider name to find its plan"))?;
    let plans = ProviderPlanStore::new(state_dir);
    let plan = plans
        .load(name)?
        .ok_or_else(|| NodError::config(format!("no recorded plan for provider `{name}`")))?;

    let report = reconcile::rollback(command, settings, &plan).await?;

    if json {
        let text = serde_json::to_string_pretty(&serde_json::json!({
            "provider": report.provider,
            "version": report.version,
            "restored": report.restored,
            "refused": report.refused,
        }))
        .map_err(|error| NodError::provider(format!("cannot render report: {error}")))?;
        println!("{text}");
    } else {
        println!("provider  {} {}", report.provider, report.version);
        println!("restored  {}", report.restored);
        println!("refused   {}", report.refused.len());
        for reason in &report.refused {
            println!("          {reason}");
        }
    }

    if report.refused.is_empty() {
        plans.clear(name)?;
    }
    record_audit(
        &report.provider,
        if report.refused.is_empty() {
            "rolled-back"
        } else {
            "rollback-refused"
        },
    )
    .await;
    Ok(())
}

/// Records a provider run in the audit trail.
///
pub(crate) async fn record_audit(provider: &str, outcome: &str) {
    use crate::domain::ports::audit_store::AuditStorePort;

    let store = crate::infrastructure::storage::json_audit_store::JsonAuditStore::from_env();
    if let Err(error) = store.record(&format!("provider:{provider}"), outcome).await {
        tracing::warn!(provider, %error, "cannot record the provider run in the audit trail");
    }
}

/// Reads a JSON file, mapping failures to configuration errors.
fn read_json(path: &Path) -> Result<serde_json::Value, NodError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|error| NodError::config(format!("cannot read {}: {error}", path.display())))?;
    serde_json::from_str(&raw)
        .map_err(|error| NodError::config(format!("cannot parse {}: {error}", path.display())))
}
