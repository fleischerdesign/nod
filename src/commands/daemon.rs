//! `nod daemon` command: the reconciler as a long-running service (ADR-028).
//!
//! Two cadences, one truth. The sweep is the authoritative cycle: it probes,
//! evaluates and deploys. The gate is a cheap remote-head check that may start
//! a sweep earlier than its cadence, so a push is noticed in seconds instead of
//! an interval. The gate never replaces a sweep: host drift happens without a
//! push, so the sweep keeps its own cadence regardless of the remote.
//!
//! Timers are monotonic. A wall clock that jumps cannot stall the loop, and a
//! suspend resumes with exactly one sweep rather than a backlog.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::Instant;

use crate::application::context::AppContext;
use crate::application::use_cases::provider_schedule::ProviderSchedule;
use crate::application::use_cases::reconcile::{self, ReconcileOptions};
use crate::commands::sync;
use crate::commands::wiring;
use crate::domain::config::CliOverrides;
use crate::domain::errors::NodError;
use crate::domain::provider::DesiredResource;
use crate::domain::watch::SyncOptions;
use crate::infrastructure::config::toml_config::TomlConfigStore;

/// Runs reconciliation cycles until the process is stopped.
pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    options: SyncOptions,
    reconcile_providers: bool,
) -> Result<(), NodError> {
    sync::validate(&options)?;
    let ctx = wiring::with_sync(ctx, &options.state_dir)?;
    let _lock = wiring::sync_lock(&options.state_dir)?;
    let ctx = Arc::new(ctx);

    let mut next_sweep = Instant::now();
    let mut next_gate = options.gate_interval_secs.gt(&0).then(Instant::now);
    // The last head the gate saw. Until the first observation there is nothing
    // to compare against, so the first tick records a baseline instead of
    // pretending a push happened.
    let mut observed_head: Option<String> = None;

    // The provider loop shares the process but not the schedule: each provider
    // has its own API and its own rate limits (ADR-030).
    let mut providers = if reconcile_providers {
        load_providers(flake_path)?
    } else {
        Vec::new()
    };

    loop {
        let mut wake = next_sweep;
        if let Some(gate) = next_gate {
            wake = wake.min(gate);
        }
        for entry in &providers {
            wake = wake.min(entry.schedule.next_run());
        }
        tokio::time::sleep_until(wake).await;

        reconcile_due(&mut providers).await;

        if let Some(gate) = next_gate {
            if gate <= Instant::now() {
                if let Ok(head) = sync::remote_head(&ctx, flake_path, &options).await {
                    if observed_head.as_deref().is_some_and(|seen| seen != head) {
                        next_sweep = Instant::now();
                    }
                    observed_head = Some(head);
                }
                // A failed gate check is not a cycle failure: the sweep reports
                // a network problem when its own turn comes, with a backoff.
                next_gate = Some(
                    Instant::now()
                        + Duration::from_secs(jittered(
                            options.gate_interval_secs,
                            options.jitter_percent,
                        )),
                );
            }
        }

        if next_sweep > Instant::now() {
            continue;
        }

        let report = match sync::cycle(&ctx, flake_path, &options).await {
            Ok(report) => report,
            Err(error) => {
                // A daemon must outlive transient failures (network, cache,
                // store lock). Report, wait, retry; never exit the service.
                tracing::error!("reconciliation cycle failed: {error}");
                let wait = jittered(options.interval_secs.max(30), options.jitter_percent);
                next_sweep = Instant::now() + Duration::from_secs(wait);
                continue;
            }
        };
        sync::emit(&report, &options);
        sync::record_audit(&ctx, &report).await;

        let wait = if report.backoff_secs > 0 {
            report.backoff_secs
        } else {
            jittered(options.interval_secs, options.jitter_percent)
        };
        next_sweep = Instant::now() + Duration::from_secs(wait);
    }
}

/// One declared provider and its cadence.
struct ProviderEntry {
    name: String,
    command: std::path::PathBuf,
    settings: serde_json::Value,
    desired: Vec<DesiredResource>,
    apply: bool,
    schedule: ProviderSchedule,
}

/// Builds the provider loop from `[providers.<name>]` in the configuration.
///
/// A provider without a `desired` file is not scheduled: the daemon has nothing
/// to compare against, and guessing a declaration would be worse than skipping.
fn load_providers(flake_path: &Path) -> Result<Vec<ProviderEntry>, NodError> {
    let store = TomlConfigStore::new(flake_path, CliOverrides::default())?;
    let now = Instant::now();
    let mut entries = Vec::new();

    for (name, provider) in store.providers() {
        let Some(desired_path) = &provider.desired else {
            tracing::warn!(
                provider = %name,
                "no desired file declared; provider is not scheduled"
            );
            continue;
        };

        let raw = std::fs::read_to_string(desired_path).map_err(|error| {
            NodError::config(format!(
                "cannot read desired state {} for provider {name}: {error}",
                desired_path.display()
            ))
        })?;
        let desired = reconcile::parse_desired(&raw)?;

        let settings = match &provider.settings {
            Some(value) => serde_json::to_value(value).map_err(|error| {
                NodError::config(format!(
                    "cannot convert settings for provider {name}: {error}"
                ))
            })?,
            None => serde_json::Value::Object(serde_json::Map::new()),
        };

        entries.push(ProviderEntry {
            name: name.clone(),
            command: provider.command.clone(),
            settings,
            desired,
            apply: provider.apply.unwrap_or(false),
            schedule: ProviderSchedule::new(
                Duration::from_secs(provider.interval_secs.unwrap_or(300)),
                now,
            ),
        });
    }

    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
}

/// Runs every provider that is due, one at a time.
///
async fn reconcile_due(entries: &mut [ProviderEntry]) {
    let now = Instant::now();

    for entry in entries.iter_mut() {
        if !entry.schedule.due(now) {
            continue;
        }

        let result = reconcile::execute(
            &entry.command,
            &entry.settings,
            &entry.desired,
            ReconcileOptions { apply: entry.apply },
        )
        .await;

        match result {
            Ok(report) => {
                tracing::info!(
                    provider = %entry.name,
                    observed = report.observed,
                    planned = report.planned,
                    approved = report.approved.len(),
                    refused = report.refused.len(),
                    applied = report.applied,
                    apply = entry.apply,
                    "provider reconcile"
                );
                entry.schedule.succeeded(Instant::now());
            }
            Err(error) => {
                let condition = match &error {
                    NodError::Auth { .. } => Some("auth"),
                    _ => None,
                };
                let (delay, changed) = entry.schedule.failed(Instant::now(), condition);
                if changed {
                    tracing::warn!(
                        provider = %entry.name,
                        %error,
                        delay_secs = delay.as_secs(),
                        "provider reconcile failed; backing off"
                    );
                } else {
                    tracing::debug!(
                        provider = %entry.name,
                        %error,
                        "provider reconcile failed again"
                    );
                }
            }
        }
    }
}

/// Spreads `seconds` by up to `percent` in either direction.
///
/// The offset is derived from the clock, not a random-number dependency: the
/// only property required is that independent hosts do not share one instant.
pub(crate) fn jittered(seconds: u64, percent: u8) -> u64 {
    if percent == 0 || seconds == 0 {
        return seconds;
    }
    let span = seconds.saturating_mul(u64::from(percent)) / 100;
    if span == 0 {
        return seconds;
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()))
        .unwrap_or(0);
    let offset = nanos % (span.saturating_mul(2).saturating_add(1));
    seconds.saturating_sub(span).saturating_add(offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_stays_within_the_configured_spread() {
        for _ in 0..50 {
            let value = jittered(100, 10);
            assert!((90..=110).contains(&value), "got {value}");
        }
    }

    #[test]
    fn zero_jitter_is_exact() {
        assert_eq!(jittered(60, 0), 60);
    }

    #[test]
    fn tiny_intervals_do_not_underflow() {
        let value = jittered(1, 50);
        assert!((0..=2).contains(&value), "got {value}");
    }

    #[test]
    fn the_gate_is_off_only_when_it_is_zero() {
        // The daemon derives its own cadence list from the options: a zero
        // gate must produce no gate deadline at all, or the loop would spin.
        let off = SyncOptions {
            gate_interval_secs: 0,
            ..SyncOptions::default()
        };
        assert!(off.gate_interval_secs.gt(&0).then(Instant::now).is_none());

        let on = SyncOptions {
            gate_interval_secs: 15,
            ..SyncOptions::default()
        };
        assert!(on.gate_interval_secs.gt(&0).then(Instant::now).is_some());
    }
}
