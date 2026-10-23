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
use crate::domain::provider::{ProviderOrder, RollbackPlan};
use crate::domain::sync::now_secs;
use crate::domain::watch::SyncOptions;
use crate::infrastructure::config::toml_config::TomlConfigStore;
use crate::infrastructure::state::provider_lock::ProviderLock;
use crate::infrastructure::storage::provider_plan::ProviderPlanStore;

/// Runs reconciliation cycles until the process is stopped.
pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    options: SyncOptions,
    reconcile_providers: bool,
    config: Option<&Path>,
) -> Result<(), NodError> {
    sync::validate(&options)?;
    let ctx = wiring::with_sync(ctx, &options.state_dir)?;
    let _lock = wiring::sync_lock(&options.state_dir)?;
    let ctx = Arc::new(ctx);

    // A host cannot verify the activation that stopped the process performing
    // it. The daemon can: it starts after that switch, so it probes the local
    // host once and records the verdict before the first cycle (ADR-029).
    match crate::application::use_cases::self_check::execute(
        &ctx,
        flake_path,
        options.gone_after_secs,
    )
    .await
    {
        Ok(Some(verdict)) => tracing::info!(?verdict, "local self-check recorded"),
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "local self-check failed"),
    }

    let mut next_sweep = Instant::now();
    let mut next_gate = options.gate_interval_secs.gt(&0).then(Instant::now);
    // The last head the gate saw. Until the first observation there is nothing
    // to compare against, so the first tick records a baseline instead of
    // pretending a push happened.
    let mut observed_head: Option<String> = None;

    // The provider loop shares the process but not the schedule: each provider
    // has its own API and its own rate limits (ADR-030).
    let mut providers = if reconcile_providers {
        load_providers(flake_path, config).await?
    } else {
        Vec::new()
    };

    // SIGHUP reloads the configuration file and the fleet declaration. Flags
    // and the environment are read at start; a running process cannot change
    // its environment.
    let mut reload = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        .map_err(|error| NodError::config(format!("cannot watch SIGHUP: {error}")))?;

    loop {
        let mut wake = next_sweep;
        if let Some(gate) = next_gate {
            wake = wake.min(gate);
        }
        for entry in &providers {
            wake = wake.min(entry.schedule.next_run());
        }
        tokio::select! {
            _ = tokio::time::sleep_until(wake) => {}
            _ = reload.recv() => {
                match load_providers(flake_path, config).await {
                    Ok(entries) => {
                        providers = entries;
                        tracing::info!(
                            providers = providers.len(),
                            "SIGHUP: fleet declaration reloaded"
                        );
                    }
                    Err(error) => tracing::error!(
                        %error,
                        "SIGHUP: reload failed; keeping the previous declaration"
                    ),
                }
                continue;
            }
        }

        // Independent providers run on their own cadence, whenever the loop wakes.
        reconcile_due(
            &mut providers,
            &options.state_dir,
            ProviderOrder::Independent,
        )
        .await;

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

        // Ordered providers run with the sweep, not beside it: a provider that
        // produces state a host needs must finish before the host is touched.
        reconcile_due(
            &mut providers,
            &options.state_dir,
            ProviderOrder::BeforeHosts,
        )
        .await;

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

        reconcile_due(
            &mut providers,
            &options.state_dir,
            ProviderOrder::AfterHosts,
        )
        .await;

        let wait = if report.backoff_secs > 0 {
            report.backoff_secs
        } else {
            jittered(options.interval_secs, options.jitter_percent)
        };
        next_sweep = Instant::now() + Duration::from_secs(wait);
    }
}

/// One declared provider and its cadence.
#[derive(Debug)]
struct ProviderEntry {
    name: String,
    command: std::path::PathBuf,
    settings: serde_json::Value,
    desired: crate::application::use_cases::reconcile::Desired,
    apply: bool,
    order: ProviderOrder,
    schedule: ProviderSchedule,
    policy: crate::domain::policy::Policy,
    env: Vec<String>,
    timeout: std::time::Duration,
}

/// Builds the provider loop from `[providers.<name>]` in the configuration.
///
/// A provider without a `desired` file is not scheduled: the daemon has nothing
/// to compare against, and guessing a declaration would be worse than skipping.
async fn load_providers(
    flake_path: &Path,
    config: Option<&Path>,
) -> Result<Vec<ProviderEntry>, NodError> {
    let store = match config {
        Some(path) => TomlConfigStore::from_file(path, CliOverrides::default())?,
        None => TomlConfigStore::new(flake_path, CliOverrides::default())?,
    };
    let fleet = crate::infrastructure::config::fleet::load(flake_path, &store).await?;
    let now = Instant::now();
    let mut entries = Vec::new();

    for (name, provider) in &fleet.providers {
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
            Some(value) => value.clone(),
            None => serde_json::Value::Object(serde_json::Map::new()),
        };

        entries.push(ProviderEntry {
            name: name.clone(),
            command: provider.command.clone(),
            settings,
            desired,
            apply: provider.apply.unwrap_or(false),
            order: provider.order.unwrap_or_default(),
            policy: provider.policy.clone().unwrap_or_default(),
            env: provider.env.clone(),
            timeout: std::time::Duration::from_secs(provider.timeout_secs.unwrap_or(60)),
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
async fn reconcile_due(entries: &mut [ProviderEntry], state_dir: &Path, order: ProviderOrder) {
    let now = Instant::now();

    for entry in entries.iter_mut() {
        if entry.order != order || !entry.schedule.due(now) {
            continue;
        }

        // A one-shot run or another daemon may hold the lock; that is not a
        // failure, it is someone else doing the work.
        let Ok(_lock) = ProviderLock::acquire(state_dir, &entry.name) else {
            tracing::debug!(
                provider = %entry.name,
                "another run holds the provider lock; deferring"
            );
            entry.schedule.deferred(Instant::now());
            continue;
        };

        let result = reconcile::execute(
            &entry.command,
            &entry.settings,
            &entry.desired,
            ReconcileOptions { apply: entry.apply },
            &entry.policy,
            &entry.env,
            entry.timeout,
        )
        .await;

        match result {
            Ok(report) => {
                tracing::info!(
                    provider = %entry.name,
                    observed = report.observed,
                    unowned = report.unowned,
                    planned = report.planned,
                    approved = report.approved.len(),
                    refused = report.refused.len(),
                    applied = report.applied,
                    failed = report.failed.len(),
                    apply = entry.apply,
                    "provider reconcile"
                );
                let outcome = if !report.refused.is_empty() {
                    "refused"
                } else if report.applied > 0 {
                    "applied"
                } else if report.planned > 0 {
                    "planned"
                } else {
                    "unchanged"
                };
                crate::commands::reconcile::record_audit(&entry.name, outcome).await;
                entry.schedule.succeeded(Instant::now());

                // Record the inverse only for runs that actually changed the
                // world; a dry run has nothing to undo.
                if entry.apply && !report.inverse.is_empty() {
                    let plans = ProviderPlanStore::new(state_dir);
                    let plan = RollbackPlan {
                        provider: report.provider.clone(),
                        applied_at: now_secs(),
                        changes: report.inverse.clone(),
                    };
                    if let Err(error) = plans.save(&plan) {
                        tracing::warn!(
                            provider = %entry.name,
                            %error,
                            "cannot record the rollback plan"
                        );
                    }
                }
            }
            Err(error) => {
                crate::commands::reconcile::record_audit(&entry.name, "failed").await;
                let (condition, retry_after) = match &error {
                    NodError::Auth { .. } => (Some("auth"), None),
                    NodError::RateLimited {
                        retry_after_secs, ..
                    } => (
                        Some("rate-limited"),
                        retry_after_secs.map(Duration::from_secs),
                    ),
                    _ => (None, None),
                };
                let (delay, changed) =
                    entry
                        .schedule
                        .failed(Instant::now(), condition, retry_after);
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

    #[tokio::test]
    async fn providers_load_from_a_file_with_every_declared_field() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("desired.json"),
            r#"{"resources":[{"kind":"example.record","id":"demo","attributes":{"value":"dry-run"}}]}"#,
        )
        .unwrap();
        let config = dir.path().join(".nod.toml");
        std::fs::write(
            &config,
            r#"
[providers.example]
command = "/bin/true"
desired = "desired.json"
apply = true
interval_secs = 120
timeout_secs = 90
env = ["NOD_TEST_TOKEN"]

[providers.example.settings]
state = "/tmp/example-state.json"

[providers.example.policy]
max_deletions = 2
protect = ["demo"]
"#,
        )
        .unwrap();

        let entries = load_providers(dir.path(), Some(&config)).await.unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.name, "example");
        assert_eq!(entry.command, std::path::PathBuf::from("/bin/true"));
        assert_eq!(entry.desired.resources.len(), 1);
        assert!(entry.apply);
        assert_eq!(entry.env, vec!["NOD_TEST_TOKEN".to_string()]);
        assert_eq!(entry.timeout, Duration::from_secs(90));
        assert_eq!(entry.policy.max_deletions, 2);
        assert!(entry.policy.is_protected("demo"));
        assert_eq!(
            entry.settings["state"].as_str(),
            Some("/tmp/example-state.json")
        );
    }

    #[tokio::test]
    async fn a_provider_without_desired_is_not_scheduled() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".nod.toml");
        std::fs::write(
            &config,
            r#"
[providers.skipped]
command = "/bin/true"
"#,
        )
        .unwrap();

        let entries = load_providers(dir.path(), Some(&config)).await.unwrap();
        assert!(entries.is_empty(), "no desired file means no schedule");
    }

    #[tokio::test]
    async fn a_missing_desired_file_names_the_provider() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".nod.toml");
        std::fs::write(
            &config,
            r#"
[providers.broken]
command = "/bin/true"
desired = "missing.json"
"#,
        )
        .unwrap();

        let error = load_providers(dir.path(), Some(&config))
            .await
            .expect_err("a missing declaration must fail the daemon start");
        assert!(error.to_string().contains("broken"), "{error}");
        assert!(error.to_string().contains("missing.json"), "{error}");
    }
}
