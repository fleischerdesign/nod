//! `nod provider adopt`: take ownership of resources that already match the
//! declaration (ADR-032).
//!
//! The core decides eligibility; the provider only writes its own marker. A
//! dry run is the default, and a conflicting resource is never taken over
//! without naming it.

use std::path::Path;
use std::time::Duration;

use crate::application::use_cases::reconcile::{check_capabilities, describe_failure};
use crate::domain::adoption::{decide, Decision};
use crate::domain::errors::NodError;
use crate::domain::provider::{DesiredResource, OwnershipMode};
use crate::infrastructure::provider::process::ProcessProvider;

/// What an adoption run should do.
#[derive(Debug, Clone, Default)]
pub struct AdoptOptions {
    /// Write the marker; without it the run only reports.
    pub apply: bool,
    /// Resources that differ from the declaration but may be taken over.
    pub force: Vec<String>,
}

/// What one adoption run produced.
#[derive(Debug, Clone, Default)]
pub struct AdoptReport {
    /// Provider name.
    pub provider: String,
    /// Provider implementation version.
    pub version: String,
    /// Resources the provider reported.
    pub observed: usize,
    /// Declared resources that match but do not carry the marker.
    pub unowned: usize,
    /// Resources this run may stamp.
    pub eligible: usize,
    /// Declared resources that differ and were not forced, with their diff.
    pub conflicts: Vec<(String, Vec<String>)>,
    /// Already-owned or undeclared resources, left alone.
    pub skipped: usize,
    /// Resources the provider confirmed as stamped.
    pub adopted: usize,
    /// Resources the provider could not stamp, with its classification.
    pub failed: Vec<String>,
}

/// Runs one adoption pass against one provider.
pub async fn execute(
    command: &Path,
    settings: &serde_json::Value,
    desired: &[DesiredResource],
    options: AdoptOptions,
    env: &[String],
    timeout: Duration,
) -> Result<AdoptReport, NodError> {
    // A forced id that names nothing declared is a typo, not a no-op.
    for forced in &options.force {
        if !desired.iter().any(|want| &want.id == forced) {
            return Err(NodError::config(format!(
                "--force {forced}: not a declared resource"
            )));
        }
    }

    let (mut provider, handshake) = ProcessProvider::open(command, timeout, env).await?;

    if handshake.ownership != OwnershipMode::Marker {
        return Err(NodError::config(format!(
            "provider {} uses ownership `{}`; adoption applies only to `marker`",
            handshake.provider,
            ownership_label(handshake.ownership)
        )));
    }
    if !handshake
        .operations
        .iter()
        .any(|operation| operation == "adopt")
    {
        return Err(NodError::config(format!(
            "provider {} does not declare the `adopt` operation",
            handshake.provider
        )));
    }

    let observed = provider.observe(settings).await?;
    check_capabilities(&handshake, &observed.resources, &[])?;
    // The provider's plan is the authority on whether a declaration is already
    // satisfied; attribute equality is not, because a seed's profile is history.
    let plan = provider
        .plan(settings, desired, &observed.resources)
        .await?;
    let changed: Vec<(String, String)> = plan
        .changes
        .iter()
        .map(|change| (change.resource.kind.clone(), change.resource.id.clone()))
        .collect();
    let candidates = decide(&observed.resources, desired, &changed);

    let mut eligible = Vec::new();
    let mut conflicts = Vec::new();
    let mut skipped = 0;
    for candidate in &candidates {
        match &candidate.decision {
            Decision::Eligible => eligible.push(candidate.resource.clone()),
            Decision::Conflict { differences } => {
                if options.force.iter().any(|id| id == &candidate.resource.id) {
                    eligible.push(candidate.resource.clone());
                } else {
                    conflicts.push((candidate.resource.id.clone(), differences.clone()));
                }
            }
            Decision::AlreadyOwned | Decision::Undeclared => skipped += 1,
        }
    }

    let mut adopted = 0;
    let mut failed = Vec::new();
    if options.apply && !eligible.is_empty() {
        let result = provider.adopt(settings, &eligible).await?;
        for resource in &eligible {
            match result.adopted.iter().find(|entry| entry.id == resource.id) {
                Some(entry) if entry.ok => adopted += 1,
                Some(entry) => failed.push(format!("{}: {}", resource.id, describe_failure(entry))),
                None => failed.push(format!("{}: the provider reported no result", resource.id)),
            }
        }
    }

    Ok(AdoptReport {
        provider: handshake.provider.clone(),
        version: handshake.version.clone(),
        observed: observed.resources.len(),
        unowned: crate::domain::adoption::unowned(&observed.resources, desired, &changed),
        eligible: eligible.len(),
        conflicts,
        skipped,
        adopted,
        failed,
    })
}

/// Human label for an ownership mode, for the error that explains why
/// adoption does not apply.
fn ownership_label(mode: OwnershipMode) -> &'static str {
    match mode {
        OwnershipMode::Marker => "marker",
        OwnershipMode::Namespace => "namespace",
        OwnershipMode::None => "none",
    }
}
