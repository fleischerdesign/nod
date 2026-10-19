//! `nod daemon` command: the reconciler as a long-running service (ADR-028).
//!
//! The loop is intentionally thin: one cycle, one report, one sleep. The
//! interval honours the cycle's own backoff, so a broken commit is not rebuilt
//! every interval, and jitter keeps many hosts from polling in lockstep.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

    loop {
        let report = match sync::cycle(&ctx, flake_path, &options).await {
            Ok(report) => report,
            Err(error) => {
                // A daemon must outlive transient failures (network, cache,
                // store lock). Report, wait, retry; never exit the service.
                tracing::error!("reconciliation cycle failed: {error}");
                let wait = jittered(options.interval_secs.max(30), options.jitter_percent);
                tokio::time::sleep(Duration::from_secs(wait)).await;
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
        tokio::time::sleep(Duration::from_secs(wait)).await;
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
}
