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
use crate::commands::sync;
use crate::commands::wiring;
use crate::domain::errors::NodError;
use crate::domain::watch::SyncOptions;

/// Runs reconciliation cycles until the process is stopped.
pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    options: SyncOptions,
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

    loop {
        let wake = next_gate.map_or(next_sweep, |gate| next_sweep.min(gate));
        tokio::time::sleep_until(wake).await;

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
