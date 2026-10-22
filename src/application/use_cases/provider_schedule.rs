//! Provider cadence for the daemon (ADR-030).
//!
//! The host sweep and the provider loop share a process but not a schedule: a
//! provider has its own API, its own rate limits and its own failure modes, so
//! it gets its own clock. The rules are pure and testable without a provider.

use std::time::Duration;

use tokio::time::Instant;

/// Upper bound for a failing provider's backoff.
pub const MAX_BACKOFF: Duration = Duration::from_secs(3600);

/// One provider's schedule.
#[derive(Debug, Clone)]
pub struct ProviderSchedule {
    interval: Duration,
    max_backoff: Duration,
    next_run: Instant,
    failures: u32,
    condition: Option<String>,
}

impl ProviderSchedule {
    /// A schedule that is due immediately.
    pub fn new(interval: Duration, now: Instant) -> Self {
        Self {
            interval,
            max_backoff: MAX_BACKOFF,
            next_run: now,
            failures: 0,
            condition: None,
        }
    }

    /// Whether the provider should run at `now`.
    pub fn due(&self, now: Instant) -> bool {
        now >= self.next_run
    }

    /// The next instant this provider is due.
    pub fn next_run(&self) -> Instant {
        self.next_run
    }

    /// The condition of the last failure, when it is still current.
    pub fn condition(&self) -> Option<&str> {
        self.condition.as_deref()
    }

    /// Records a successful cycle: the next run is one interval away.
    pub fn succeeded(&mut self, now: Instant) {
        self.failures = 0;
        self.condition = None;
        self.next_run = now + self.interval;
    }

    /// Defers the next run by one interval without touching the failure count.
    ///
    /// Used when another process holds the provider's lock: the work is being
    /// done, so this is neither a success nor a failure.
    pub fn deferred(&mut self, now: Instant) {
        self.next_run = now + self.interval;
    }

    /// Records a failure and returns the delay plus whether the condition
    /// changed since the previous failure.
    ///
    /// A provider that reports a rate limit knows better than we do when it is
    /// ready: its `retry_after` wins over the doubling, bounded by the cap.
    /// Doubling keeps a flapping provider from hammering an API; an auth
    /// failure goes straight to the cap, because waiting will not fix a token
    /// and retrying every interval only fills the log.
    pub fn failed(
        &mut self,
        now: Instant,
        condition: Option<&str>,
        retry_after: Option<Duration>,
    ) -> (Duration, bool) {
        self.failures = self.failures.saturating_add(1);
        let changed = self.condition.as_deref() != condition;
        self.condition = condition.map(str::to_string);

        let delay = if condition == Some("auth") {
            self.max_backoff
        } else if let Some(hint) = retry_after {
            hint.min(self.max_backoff)
        } else {
            let factor = 1u32 << self.failures.min(6);
            self.interval.saturating_mul(factor).min(self.max_backoff)
        };

        self.next_run = now + delay;
        (delay, changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule() -> ProviderSchedule {
        ProviderSchedule::new(Duration::from_secs(60), Instant::now())
    }

    #[test]
    fn a_new_schedule_is_due_immediately() {
        let schedule = schedule();
        assert!(schedule.due(Instant::now()));
    }

    #[test]
    fn success_schedules_one_interval_ahead() {
        let mut schedule = schedule();
        let now = Instant::now();
        schedule.succeeded(now);
        assert!(!schedule.due(now));
        assert!(schedule.due(now + Duration::from_secs(60)));
    }

    #[test]
    fn failures_double_until_the_cap() {
        let mut schedule = schedule();
        let now = Instant::now();

        let (first, _) = schedule.failed(now, None, None);
        let (second, _) = schedule.failed(now, None, None);
        let (third, _) = schedule.failed(now, None, None);

        assert_eq!(first, Duration::from_secs(120));
        assert_eq!(second, Duration::from_secs(240));
        assert_eq!(third, Duration::from_secs(480));
    }

    #[test]
    fn the_backoff_never_exceeds_the_cap() {
        let mut schedule = schedule();
        let now = Instant::now();
        let mut last = Duration::ZERO;
        for _ in 0..12 {
            last = schedule.failed(now, None, None).0;
        }
        assert_eq!(last, MAX_BACKOFF);
    }

    #[test]
    fn an_auth_failure_goes_straight_to_the_cap_and_reports_once() {
        let mut schedule = schedule();
        let now = Instant::now();

        let (delay, changed) = schedule.failed(now, Some("auth"), None);
        assert_eq!(delay, MAX_BACKOFF);
        assert!(changed);

        let (_, changed_again) = schedule.failed(now, Some("auth"), None);
        assert!(!changed_again, "the same condition must be logged once");
        assert_eq!(schedule.condition(), Some("auth"));
    }

    #[test]
    fn a_success_clears_the_condition() {
        let mut schedule = schedule();
        schedule.failed(Instant::now(), Some("auth"), None);
        schedule.succeeded(Instant::now());
        assert_eq!(schedule.condition(), None);
    }

    #[test]
    fn a_deferral_keeps_the_failure_count() {
        let mut schedule = schedule();
        let now = Instant::now();
        schedule.failed(now, None, None);
        schedule.deferred(now);
        assert!(!schedule.due(now));
        assert!(schedule.due(now + Duration::from_secs(60)));
        assert_eq!(schedule.condition(), None);
    }

    #[test]
    fn a_rate_limit_hint_wins_over_the_doubling() {
        let mut schedule = schedule();
        let (delay, _) = schedule.failed(
            Instant::now(),
            Some("rate-limited"),
            Some(Duration::from_secs(90)),
        );
        assert_eq!(delay, Duration::from_secs(90));
    }

    #[test]
    fn a_rate_limit_hint_is_still_bounded_by_the_cap() {
        let mut schedule = schedule();
        let (delay, _) = schedule.failed(
            Instant::now(),
            Some("rate-limited"),
            Some(Duration::from_secs(99_999)),
        );
        assert_eq!(delay, MAX_BACKOFF);
    }
}
