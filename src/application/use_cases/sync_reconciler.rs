//! `SyncReconcilerUseCase`: reconcile one branch into the running fleet
//! (ADR-028).
//!
//! One cycle answers a single question — does every selected host run the
//! closure of the branch head? — and acts only on the difference:
//!
//! ```text
//! preflight (lock, dirty) → ls-remote gate → fetch → signature → ancestry
//!   → fast-forward → per-host drift → deploy (waves, health, rollback)
//!   → report → state → notify
//! ```
//!
//! The applied state is derived from closures; [`SyncState`] only remembers
//! what was attempted, which drives the backoff and crash recovery.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::application::context::AppContext;
use crate::application::pipeline::state_machine::DeploymentState;
use crate::application::use_cases::deploy_fleet::{DeployFleetUseCase, HostOutcome};
use crate::application::use_cases::sync_inspect::{inspect_hosts, Reachability};
use crate::domain::errors::NodError;
use crate::domain::health::HealthVerdict;
use crate::domain::host::HostEntity;
use crate::domain::plan::{DeploymentAction, DeploymentOptions, RolloutStrategy};
use crate::domain::sync::{
    now_secs, HostCondition, HostMemory, RetryPolicy, SyncHostReport, SyncHostResult,
    SyncPlanEntry, SyncReport, SyncState,
};
use crate::domain::watch::SyncOptions;

/// Reconciles a repository into the running fleet.
pub struct SyncReconcilerUseCase {
    ctx: Arc<AppContext>,
}

impl SyncReconcilerUseCase {
    /// Builds the use case over a seeded context.
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    /// Runs one reconciliation cycle.
    ///
    /// `select_hosts` is invoked only after the cycle decided to act, so a
    /// polling cycle that finds no new revision never pays for host discovery
    /// or flake evaluation.
    pub async fn reconcile_once<F, Fut>(
        &self,
        flake_path: &Path,
        options: &SyncOptions,
        select_hosts: F,
    ) -> Result<SyncReport, NodError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<HostEntity>, NodError>>,
    {
        let git = self.ctx.git_port()?;
        let state_store = self.ctx.sync_state()?;
        let now = now_secs();

        let mut state = match state_store.load().await? {
            Some(state) => state,
            None => SyncState::new(
                format!("{}/{}", options.remote, options.branch),
                options.branch.clone(),
            ),
        };

        if state.backoff_remaining(now) > 0 {
            let mut report = SyncReport::refused(
                state
                    .last_attempt_rev
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                "backing off after a failed cycle",
            );
            report.backoff_secs = state.backoff_remaining(now);
            return Ok(report);
        }

        if git.is_dirty(flake_path).await? {
            return Ok(SyncReport::refused(
                git.local_head(flake_path).await.unwrap_or_default(),
                "worktree has uncommitted changes; sync deploys commits only",
            ));
        }

        let local_before = git.local_head(flake_path).await?;
        let remote = match git
            .remote_head(flake_path, &options.remote, &options.branch)
            .await
        {
            Ok(revision) => revision,
            Err(error) => {
                // A network failure is a failed cycle, not a refused one: it
                // backs off and reports instead of flooding the next interval.
                let backoff = state.record_failure(now, local_before.clone());
                state_store.save(&state).await?;
                let mut report = SyncReport::failed(local_before, error.to_string());
                report.backoff_secs = backoff;
                self.notify(&report, options).await;
                return Ok(report);
            }
        };

        // A cycle is a no-op only when the revision is unchanged *and* the last
        // applied revision proves the fleet was reconciled at it. A missing or
        // interrupted marker forces a drift check instead of an assumption.
        let recovered = state.in_progress.is_some();
        let revision_unchanged = remote == local_before;
        let proven_applied = state.last_applied_rev.as_deref() == Some(local_before.as_str());
        let mut selected: Option<Vec<HostEntity>> = None;

        if revision_unchanged && proven_applied && !recovered {
            let policy = RetryPolicy::from_options(options);
            let away: Vec<(&String, &HostMemory)> = state
                .hosts
                .iter()
                .filter(|(_, memory)| memory.condition != HostCondition::Reachable)
                .collect();

            if away.is_empty() {
                let verified = state.last_verified_rev.as_deref() == Some(local_before.as_str());
                let report = SyncReport {
                    revision: local_before.clone(),
                    previous_revision: Some(local_before),
                    applied: true,
                    verified,
                    hosts: Vec::new(),
                    refused: None,
                    error: None,
                    backoff_secs: 0,
                    plan: Vec::new(),
                };
                if !options.dry_run {
                    state.record_success(report.revision.clone());
                    state_store.save(&state).await?;
                    self.notify(&report, options).await;
                }
                return Ok(report);
            }

            // Retry only the hosts whose retry is due. A host that is simply
            // off costs one probe per backoff step, not one per cycle.
            let hosts = select_hosts().await?;
            let mut any_back = false;
            for host in &hosts {
                let due = match state.host(&host.name).map(|m| &m.condition) {
                    Some(HostCondition::Offline { next_probe_at, .. }) => *next_probe_at <= now,
                    Some(HostCondition::Misconfigured { .. }) => true,
                    Some(HostCondition::Reachable) | None => false,
                };
                if !due {
                    continue;
                }
                match self.ctx.deployer_for(host).check_reachability(host).await {
                    Ok(true) => {
                        state.observe_reachable(&host.name, now);
                        any_back = true;
                    }
                    Ok(false) => {
                        state.observe_offline(&host.name, now, &policy);
                    }
                    Err(error) => {
                        Self::observe_probe_error(&mut state, &host.name, now, &policy, &error);
                    }
                }
            }
            if !options.dry_run {
                state_store.save(&state).await?;
            }

            if !any_back {
                let report = SyncReport {
                    revision: local_before.clone(),
                    previous_revision: Some(local_before),
                    applied: false,
                    verified: false,
                    hosts: state
                        .hosts
                        .iter()
                        .filter(|(_, memory)| memory.condition != HostCondition::Reachable)
                        .map(|(name, memory)| {
                            let detail = match &memory.condition {
                                HostCondition::Offline {
                                    since, attempts, ..
                                } => format!("offline since {since} ({attempts} probe(s) failed)"),
                                HostCondition::Misconfigured { detail, .. } => detail.clone(),
                                HostCondition::Reachable => String::new(),
                            };
                            SyncHostReport::new(name.clone(), SyncHostResult::Unreachable)
                                .with_detail(detail)
                        })
                        .collect(),
                    refused: None,
                    error: None,
                    backoff_secs: 0,
                    plan: Vec::new(),
                };
                // A cycle that only confirmed known outages is not a success and
                // not a failure: nothing about the fleet changed.
                self.notify(&report, options).await;
                return Ok(report);
            }

            // A host answered again: fall through to a full check with the
            // host list already in hand.
            selected = Some(hosts);
        }

        if revision_unchanged {
            // Recovered cycle or first observation: verify drifts below.
        } else {
            if options.dry_run {
                // A preview writes nothing, including the checkout: integrating
                // the remote here would be a side effect the caller did not ask
                // for and, for a non-root caller, an impossible one.
                return Ok(SyncReport::refused(
                    local_before,
                    format!("checkout is behind {remote}; a preview does not integrate"),
                ));
            }
            if let Err(error) = git
                .fetch(flake_path, &options.remote, &options.branch)
                .await
            {
                let backoff = state.record_failure(now, remote.clone());
                state_store.save(&state).await?;
                let mut report = SyncReport::failed(local_before, error.to_string());
                report.backoff_secs = backoff;
                self.notify(&report, options).await;
                return Ok(report);
            }

            if options.require_signature && !git.verify_signature(flake_path, &remote).await? {
                return Ok(SyncReport::refused(
                    local_before,
                    "integrated revision has no verified signature",
                ));
            }

            if !options.allow_downgrade {
                if let Some(applied) = state.last_applied_rev.as_deref() {
                    if !git.is_ancestor(flake_path, applied, &remote).await? {
                        return Ok(SyncReport::refused(
                            local_before,
                            format!(
                                "refusing downgrade: {remote} is not a descendant of {applied}"
                            ),
                        ));
                    }
                }
            }
        }

        let head = if revision_unchanged {
            local_before.clone()
        } else {
            git.fast_forward(flake_path, &options.remote, &options.branch)
                .await?
        };

        let hosts = match selected {
            Some(hosts) => hosts,
            None => select_hosts().await?,
        };
        if hosts.is_empty() {
            return Err(NodError::config(
                "no hosts selected; pass --target/--all or set [sync].targets",
            ));
        }

        let self_host = hosts
            .iter()
            .find(|host| host.is_self)
            .map(|host| host.name.clone());
        let retry = RetryPolicy::from_options(options);
        // Hosts known to be away are deferred until their retry is due, so an
        // offline laptop costs no probe instead of one per cycle.
        let policy = state.probe_policy(now, &retry);
        let inspections = inspect_hosts(
            &self.ctx,
            flake_path,
            hosts,
            options.concurrency,
            options.verbose,
            &policy,
        )
        .await?;

        let mut changed = Vec::new();
        let mut reports = Vec::new();
        let mut plan = Vec::new();
        for inspection in inspections {
            match &inspection.reachability {
                Reachability::Deferred { reason } => {
                    // Not probed: what the last cycle learned says the retry is
                    // not due yet. The detail carries the observation that
                    // would otherwise be re-measured every interval.
                    reports.push(
                        SyncHostReport::new(
                            inspection.host.name.clone(),
                            SyncHostResult::Unreachable,
                        )
                        .with_detail(format!("deferred: {reason}")),
                    );
                }
                Reachability::Misconfigured { detail } => {
                    if !options.dry_run {
                        state.observe_misconfigured(&inspection.host.name, now, detail.clone());
                    }
                    reports.push(
                        SyncHostReport::new(
                            inspection.host.name.clone(),
                            SyncHostResult::Unreachable,
                        )
                        .with_detail(detail.clone()),
                    );
                }
                Reachability::Unreachable { detail } => {
                    if !options.dry_run {
                        state.observe_offline(&inspection.host.name, now, &retry);
                    }
                    let mut report = SyncHostReport::new(
                        inspection.host.name.clone(),
                        SyncHostResult::Unreachable,
                    );
                    if let Some(detail) = detail {
                        report = report.with_detail(detail.clone());
                    }
                    reports.push(report);
                }
                Reachability::Reachable => {
                    if !options.dry_run {
                        state.observe_reachable(&inspection.host.name, now);
                    }
                    if inspection.drifted() {
                        if options.plan || options.dry_run {
                            plan.push(SyncPlanEntry {
                                host: inspection.host.name.clone(),
                                current: inspection.current.clone(),
                                target: inspection.target.clone().unwrap_or_default(),
                                units: None,
                            });
                        }
                        changed.push(inspection.host);
                    } else {
                        reports.push(SyncHostReport::new(
                            inspection.host.name.clone(),
                            SyncHostResult::Unchanged,
                        ));
                    }
                }
            }
        }
        changed = Self::order_last(changed, &options.last_hosts);

        // Under `canary` the first host of the first wave is the probe host
        // (ADR-005). No election is needed, and none is written: only hosts
        // that answered and drifted enter `changed`, so an unreachable canary
        // cannot exist and cannot block the rollout. Naming it is enough - an
        // operator reading the journal should not have to infer the order.
        if matches!(
            RolloutStrategy::parse(&options.strategy),
            Some(RolloutStrategy::Canary)
        ) {
            if let Some(first) = changed.first() {
                tracing::info!(
                    canary = %first.name,
                    "canary selected from the hosts that must change"
                );
            }
        }

        if options.dry_run {
            for host in &changed {
                reports.push(SyncHostReport::new(
                    host.name.clone(),
                    SyncHostResult::Skipped,
                ));
            }
            if options.plan {
                // Unit detail is only honest for a closure that exists on this
                // host: `dry-activate` is a local operation, and copying a
                // closure to a remote host to ask it would be a deployment.
                if let Some(self_host) = self_host.as_deref() {
                    for entry in plan.iter_mut().filter(|entry| entry.host == self_host) {
                        entry.units = Self::activation_units(&entry.target).await;
                    }
                }
            }
            let report = SyncReport {
                revision: head.clone(),
                previous_revision: Some(local_before.clone()),
                applied: false,
                verified: false,
                hosts: reports,
                refused: None,
                error: None,
                backoff_secs: 0,
                plan,
            };
            // A preview also does not run notify hooks: they are deployment
            // side effects, not part of an answer about what would happen.
            return Ok(report);
        }

        if changed.is_empty() {
            let settled = reports.iter().all(|host| host.result.is_settled());
            // An idle cycle has no fresh observation, so it can preserve a
            // verified marker but never create one.
            let verified = settled && state.last_verified_rev.as_deref() == Some(head.as_str());
            let report = SyncReport {
                revision: head.clone(),
                previous_revision: Some(local_before),
                applied: settled,
                verified,
                hosts: reports,
                refused: None,
                error: None,
                backoff_secs: 0,
                plan: Vec::new(),
            };
            if settled && verified {
                state.record_success(head);
            } else {
                // Nothing to deploy, but some host did not answer or could not
                // be verified. `record_deployed` keeps the revision as reached
                // without claiming the fleet is verified at it.
                let revision = head;
                state.last_attempt_rev = Some(revision.clone());
                state.record_deployed(revision);
            }
            state_store.save(&state).await?;
            self.notify(&report, options).await;
            return Ok(report);
        }

        state.begin_progress(
            head.clone(),
            changed.iter().map(|h| h.name.clone()).collect(),
            now,
        );
        state_store.save(&state).await?;

        let deployment = Self::deployment_options(options)?;
        let summary = DeployFleetUseCase::new(self.ctx.clone())
            .execute(changed, deployment, flake_path)
            .await?;

        let mut failed = false;
        for outcome in &summary.outcomes {
            let mapped = Self::map_outcome(outcome);
            if mapped.result.is_failure() {
                failed = true;
            }
            reports.push(mapped);
        }

        let settled = reports.iter().all(|host| host.result.is_settled());
        let applied = settled && !failed;
        let verified = Self::verified(&reports);
        for report in &reports {
            if let Some(verdict) = &report.health {
                state.record_health(&report.host, verdict.clone(), now);
            }
        }
        let mut report = SyncReport {
            revision: head.clone(),
            previous_revision: Some(local_before),
            applied,
            verified,
            hosts: reports,
            refused: None,
            error: None,
            backoff_secs: 0,
            plan: Vec::new(),
        };

        if failed {
            report.backoff_secs = state.record_failure(now_secs(), head.clone());
            report.error = Some(format!(
                "{} host(s) failed",
                report
                    .hosts
                    .iter()
                    .filter(|h| h.result.is_failure())
                    .count()
            ));
        } else if verified {
            // Every host answered and is verified healthy: the fleet is at the
            // revision in the strong sense.
            state.record_success(head);
        } else {
            // A host that did not answer is not a failed cycle: the revision
            // reached every host that answered. It is not a verified cycle
            // either — `record_deployed` keeps that distinction.
            state.record_deployed(head);
        }
        state_store.save(&state).await?;
        self.notify(&report, options).await;

        Ok(report)
    }

    /// Builds the deployment policy from the sync options.
    fn deployment_options(options: &SyncOptions) -> Result<DeploymentOptions, NodError> {
        let strategy = RolloutStrategy::parse(&options.strategy)
            .ok_or_else(|| NodError::config(format!("unknown strategy '{}'", options.strategy)))?;
        if options.concurrency == 0 {
            return Err(NodError::config("--concurrency must be at least 1"));
        }
        Ok(DeploymentOptions {
            concurrency: options.concurrency,
            strategy,
            batch_size: options.batch_size,
            fail_fast: options.fail_fast,
            auto_rollback: options.auto_rollback,
            dry_run: false,
            action: DeploymentAction::Switch,
            verbose: options.verbose,
            out_link: None,
            builder: None,
        })
    }

    /// Builds every host's toplevel and compares it with the live closure.
    ///
    /// Health probing, evaluation and closure comparison of independent hosts
    /// run concurrently under the same `--concurrency` budget the rollout uses;
    /// reports are sorted by host name so the output order does not depend on
    /// completion order. A build failure aborts the cycle: partial drift
    /// information would make the report lie about what the fleet runs.
    /// Runs the target closure's `dry-activate` and keeps the unit lines.
    ///
    /// Only meaningful for a closure present on this host; a remote host would
    /// have to receive the closure first, which is a deployment rather than a
    /// plan. A failure to run it is not a plan failure: the entry simply has no
    /// unit detail.
    async fn activation_units(closure: &Path) -> Option<String> {
        let binary = closure.join("bin/switch-to-configuration");
        if !binary.exists() {
            return None;
        }
        let output = Command::new(binary)
            .arg("dry-activate")
            .output()
            .await
            .ok()?;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push('\n');
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        Self::unit_summary(&text)
    }

    /// Keeps the lines that name a unit or a lifecycle action.
    fn unit_summary(output: &str) -> Option<String> {
        let mut lines: Vec<&str> = output
            .lines()
            .map(str::trim)
            .filter(|line| {
                !line.is_empty()
                    && (line.contains("unit")
                        || line.contains("restart")
                        || line.contains("start")
                        || line.contains("stop"))
            })
            .collect();
        lines.dedup();
        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }

    /// Orders hosts so infrastructure the reconciler needs is deployed last.
    /// Hosts in `last_hosts` and the executing host declare a dependency on
    /// every other target, which the existing DAG planner turns into a final
    /// wave.
    /// Records what a failed probe means for host memory.
    ///
    /// A refusal is stored as a defect, an unanswered probe as an outage: the
    /// operator has to be able to tell "the key is wrong" from "the machine is
    /// off", and only one of those resolves itself with time.
    fn observe_probe_error(
        state: &mut SyncState,
        host: &str,
        now: u64,
        policy: &RetryPolicy,
        error: &NodError,
    ) {
        match error {
            NodError::Auth { detail } => state.observe_misconfigured(host, now, detail.clone()),
            _ => {
                state.observe_offline(host, now, policy);
            }
        }
    }

    fn order_last(mut hosts: Vec<HostEntity>, last_hosts: &[String]) -> Vec<HostEntity> {
        let last: Vec<String> = hosts
            .iter()
            .filter(|h| h.is_self || last_hosts.iter().any(|name| name == &h.name))
            .map(|h| h.name.clone())
            .collect();
        if last.is_empty() {
            return hosts;
        }
        let others: Vec<String> = hosts
            .iter()
            .filter(|h| !last.contains(&h.name))
            .map(|h| h.name.clone())
            .collect();
        for host in hosts.iter_mut() {
            if last.contains(&host.name) {
                for name in &others {
                    if !host.nod_config.rollout.depends_on.contains(name) {
                        host.nod_config.rollout.depends_on.push(name.clone());
                    }
                }
            }
        }
        hosts
    }

    /// Maps a pipeline outcome to the sync report vocabulary.
    ///
    /// A degraded host is a failed cycle (ADR-029 §6): the activation returned
    /// zero, but the deployment broke something, and calling that `deployed`
    /// would make the report green about its own damage.
    fn map_outcome(outcome: &HostOutcome) -> SyncHostReport {
        let mut result = match outcome.state {
            DeploymentState::Completed => SyncHostResult::Deployed,
            DeploymentState::RolledBack => SyncHostResult::RolledBack,
            DeploymentState::Failed => SyncHostResult::Failed,
            _ => SyncHostResult::Skipped,
        };
        let mut detail = outcome.failure.clone();
        match &outcome.health_verdict {
            Some(HealthVerdict::Degraded { units }) => {
                result = SyncHostResult::Failed;
                detail = Some(format!("degraded after activation: {}", units.join(", ")));
            }
            Some(HealthVerdict::Unverified { reason }) => {
                detail = Some(format!("unverified: {reason}"));
            }
            Some(HealthVerdict::PendingReboot) => {
                detail = Some("reboot required before the change takes effect".to_string());
            }
            Some(HealthVerdict::DegradedPreExisting { units }) => {
                detail = Some(format!("pre-existing degradation: {}", units.join(", ")));
            }
            Some(HealthVerdict::Healthy) | None => {}
        }

        let mut report = SyncHostReport::new(outcome.host_name.clone(), result)
            .with_health(outcome.health_verdict.clone());
        if let Some(detail) = detail {
            report = report.with_detail(detail);
        }
        report
    }

    /// A cycle is verified when every selected host is settled and its health
    /// verdict from this cycle is `healthy`. `pending-reboot` and `unverified`
    /// are not converged (ADR-029 §1). A remembered verdict does not count:
    /// verification is a statement about observations, not about memory.
    fn verified(reports: &[SyncHostReport]) -> bool {
        !reports.is_empty()
            && reports.iter().all(|report| {
                report.result.is_settled() && matches!(report.health, Some(HealthVerdict::Healthy))
            })
    }

    /// Runs the configured notification commands; failures are logged, never
    /// allowed to fail the cycle.
    async fn notify(&self, report: &SyncReport, options: &SyncOptions) {
        if options.notify.is_empty() {
            return;
        }
        let payload = match serde_json::to_vec(report) {
            Ok(payload) => payload,
            Err(e) => {
                tracing::warn!("cannot serialize sync report for notify hooks: {e}");
                return;
            }
        };

        for command in &options.notify {
            let mut child = match Command::new("sh")
                .arg("-c")
                .arg(command)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(e) => {
                    tracing::warn!("cannot start notify hook '{command}': {e}");
                    continue;
                }
            };
            if let Some(mut stdin) = child.stdin.take() {
                if let Err(e) = stdin.write_all(&payload).await {
                    tracing::warn!("cannot write to notify hook '{command}': {e}");
                }
            }
            if let Err(e) = child.wait().await {
                tracing::warn!("notify hook '{command}' did not complete: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::health::DeploymentReport;
    use crate::domain::host::{BuilderHost, SshProfile};
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use crate::domain::ports::git::GitPort;
    use crate::domain::ports::sync_state::SyncStatePort;
    use async_trait::async_trait;
    use mockall::mock;
    use std::path::PathBuf;

    mock! {
        Git {}
        #[async_trait]
        impl GitPort for Git {
            async fn is_dirty(&self, repo: &Path) -> Result<bool, NodError>;
            async fn local_head(&self, repo: &Path) -> Result<String, NodError>;
            async fn remote_head(&self, repo: &Path, remote: &str, branch: &str) -> Result<String, NodError>;
            async fn fetch(&self, repo: &Path, remote: &str, branch: &str) -> Result<(), NodError>;
            async fn fast_forward(&self, repo: &Path, remote: &str, branch: &str) -> Result<String, NodError>;
            async fn is_ancestor(&self, repo: &Path, ancestor: &str, descendant: &str) -> Result<bool, NodError>;
            async fn verify_signature(&self, repo: &Path, rev: &str) -> Result<bool, NodError>;
        }
    }

    mock! {
        State {}
        #[async_trait]
        impl SyncStatePort for State {
            async fn load(&self) -> Result<Option<SyncState>, NodError>;
            async fn save(&self, state: &SyncState) -> Result<(), NodError>;
        }
    }

    mock! {
        Deployer {}
        #[async_trait]
        impl DeployerPort for Deployer {
            async fn check_reachability(&self, host: &HostEntity) -> Result<bool, NodError>;
            async fn current_closure(&self, host: &HostEntity, profile: &SshProfile) -> Result<Option<PathBuf>, NodError>;
            async fn deploy_and_activate(&self, host: &HostEntity, profile: &SshProfile, closure: &Path, action: &str, verbose: bool) -> Result<DeploymentReport, NodError>;
            async fn rollback(&self, host: &HostEntity, profile: &SshProfile) -> Result<(), NodError>;
        }
    }

    mock! {
        Evaluator {}
        #[async_trait]
        impl EvaluatorPort for Evaluator {
            async fn discover_hosts(&self, flake_path: &Path, verbose: bool) -> Result<Vec<HostEntity>, NodError>;
            async fn build_toplevel<'a>(&self, flake_path: &Path, host_name: &str, closure_attr: Option<String>, builder: Option<&'a BuilderHost>, verbose: bool) -> Result<PathBuf, NodError>;
        }
    }

    #[test]
    fn deployment_options_reject_unknown_strategy() {
        let options = SyncOptions {
            strategy: "nope".to_string(),
            ..Default::default()
        };
        assert!(SyncReconcilerUseCase::deployment_options(&options).is_err());
    }

    #[test]
    fn deployment_options_reject_zero_concurrency() {
        let options = SyncOptions {
            concurrency: 0,
            ..Default::default()
        };
        assert!(SyncReconcilerUseCase::deployment_options(&options).is_err());
    }

    #[test]
    fn outcomes_map_to_settled_results() {
        let completed = HostOutcome::new("a", DeploymentState::Completed);
        assert_eq!(
            SyncReconcilerUseCase::map_outcome(&completed).result,
            SyncHostResult::Deployed
        );
        let failed = HostOutcome::new("b", DeploymentState::Failed);
        assert_eq!(
            SyncReconcilerUseCase::map_outcome(&failed).result,
            SyncHostResult::Failed
        );
    }

    #[test]
    fn a_degraded_activation_maps_to_a_failed_cycle() {
        use crate::domain::health::HealthVerdict;

        let mut outcome = HostOutcome::new("a", DeploymentState::Completed);
        outcome.health_verdict = Some(HealthVerdict::Degraded {
            units: vec!["crowdsec.service".to_string()],
        });

        let report = SyncReconcilerUseCase::map_outcome(&outcome);
        assert_eq!(report.result, SyncHostResult::Failed);
        assert!(report
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("crowdsec"));
        assert_eq!(report.health, outcome.health_verdict);
    }

    #[test]
    fn only_healthy_settled_hosts_make_a_cycle_verified() {
        use crate::domain::health::HealthVerdict;

        let healthy = SyncHostReport::new("a", SyncHostResult::Deployed)
            .with_health(Some(HealthVerdict::Healthy));
        assert!(SyncReconcilerUseCase::verified(&[healthy]));

        let pending = SyncHostReport::new("a", SyncHostResult::Deployed)
            .with_health(Some(HealthVerdict::PendingReboot));
        assert!(!SyncReconcilerUseCase::verified(&[pending]));

        let unknown = SyncHostReport::new("a", SyncHostResult::Deployed);
        assert!(!SyncReconcilerUseCase::verified(&[unknown]));
    }

    #[test]
    fn an_idle_cycle_preserves_but_does_not_create_a_verified_marker() {
        let mut state = SyncState::new("repo", "main");
        assert_ne!(state.last_verified_rev.as_deref(), Some("abc"));

        state.record_success("abc");
        assert_eq!(state.last_verified_rev.as_deref(), Some("abc"));

        // A later idle cycle must not clear it.
        state.record_deployed("abc");
        assert_eq!(state.last_verified_rev.as_deref(), Some("abc"));
    }

    #[test]
    fn self_host_moves_behind_the_others() {
        let mut a = HostEntity::new("a", "10.0.0.1", false);
        let self_host = HostEntity::new("gateway", "10.0.0.2", true);
        a.nod_config.rollout.depends_on = Vec::new();
        let ordered = SyncReconcilerUseCase::order_last(vec![self_host, a], &[]);
        let gateway = ordered.iter().find(|h| h.name == "gateway").unwrap();
        assert!(gateway
            .nod_config
            .rollout
            .depends_on
            .contains(&"a".to_string()));
    }

    fn host() -> HostEntity {
        HostEntity::new("atlas", "10.0.0.8", true)
    }

    fn options() -> SyncOptions {
        SyncOptions {
            targets: vec!["atlas".to_string()],
            ..Default::default()
        }
    }

    fn context(
        git: MockGit,
        store: MockState,
        evaluator: MockEvaluator,
        deployer: MockDeployer,
    ) -> Arc<AppContext> {
        Arc::new(
            AppContext::new(
                Arc::new(evaluator),
                Arc::new(deployer),
                Arc::new(MockDeployer::new()),
            )
            .with_git_port(Arc::new(git))
            .with_sync_state(Arc::new(store)),
        )
    }

    #[tokio::test]
    async fn an_unchanged_proven_revision_does_not_build_or_deploy() {
        let mut git = MockGit::new();
        git.expect_is_dirty().returning(|_| Ok(false));
        git.expect_local_head().returning(|_| Ok("aaa".into()));
        git.expect_remote_head()
            .returning(|_, _, _| Ok("aaa".into()));

        let mut state = SyncState::new("origin/main", "main");
        state.record_success("aaa");
        let mut store = MockState::new();
        store
            .expect_load()
            .returning(move || Ok(Some(state.clone())));
        store.expect_save().returning(|_| Ok(()));

        let mut evaluator = MockEvaluator::new();
        evaluator.expect_build_toplevel().times(0);

        let ctx = context(git, store, evaluator, MockDeployer::new());
        let report = SyncReconcilerUseCase::new(ctx)
            .reconcile_once(Path::new("/repo"), &options(), || async {
                Ok(vec![host()])
            })
            .await
            .unwrap();

        assert!(report.applied);
        assert!(report.hosts.is_empty());
    }

    #[tokio::test]
    async fn a_new_revision_deploys_only_the_drifted_host() {
        let mut git = MockGit::new();
        git.expect_is_dirty().returning(|_| Ok(false));
        git.expect_local_head().returning(|_| Ok("aaa".into()));
        git.expect_remote_head()
            .returning(|_, _, _| Ok("bbb".into()));
        git.expect_fetch().returning(|_, _, _| Ok(()));
        git.expect_is_ancestor().returning(|_, _, _| Ok(true));
        git.expect_fast_forward()
            .returning(|_, _, _| Ok("bbb".into()));

        let mut state = SyncState::new("origin/main", "main");
        state.record_success("aaa");
        let mut store = MockState::new();
        store
            .expect_load()
            .returning(move || Ok(Some(state.clone())));
        store.expect_save().returning(|_| Ok(()));

        let mut evaluator = MockEvaluator::new();
        evaluator
            .expect_build_toplevel()
            .returning(|_, _, _, _, _| Ok(PathBuf::from("/nix/store/new")));

        let mut deployer = MockDeployer::new();
        deployer.expect_check_reachability().returning(|_| Ok(true));
        deployer
            .expect_current_closure()
            .returning(|_, _| Ok(Some(PathBuf::from("/nix/store/old"))));
        deployer
            .expect_deploy_and_activate()
            .times(1)
            .returning(|_, _, _, _, _| Ok(DeploymentReport::healthy()));

        let ctx = context(git, store, evaluator, deployer);
        let report = SyncReconcilerUseCase::new(ctx)
            .reconcile_once(Path::new("/repo"), &options(), || async {
                Ok(vec![host()])
            })
            .await
            .unwrap();

        assert!(report.applied, "report: {report:?}");
        assert_eq!(report.deployed().len(), 1);
    }

    #[tokio::test]
    async fn a_dirty_worktree_is_refused_before_any_build() {
        let mut git = MockGit::new();
        git.expect_is_dirty().returning(|_| Ok(true));
        git.expect_local_head().returning(|_| Ok("aaa".into()));

        let mut store = MockState::new();
        store.expect_load().returning(|| Ok(None));

        let mut evaluator = MockEvaluator::new();
        evaluator.expect_build_toplevel().times(0);

        let ctx = context(git, store, evaluator, MockDeployer::new());
        let report = SyncReconcilerUseCase::new(ctx)
            .reconcile_once(Path::new("/repo"), &options(), || async {
                Ok(vec![host()])
            })
            .await
            .unwrap();

        assert!(report.refused.is_some());
        assert!(!report.applied);
    }

    #[tokio::test]
    async fn an_older_target_is_refused_without_activating() {
        let mut git = MockGit::new();
        git.expect_is_dirty().returning(|_| Ok(false));
        git.expect_local_head().returning(|_| Ok("aaa".into()));
        git.expect_remote_head()
            .returning(|_, _, _| Ok("old".into()));
        git.expect_fetch().returning(|_, _, _| Ok(()));
        git.expect_is_ancestor().returning(|_, _, _| Ok(false));

        let mut state = SyncState::new("origin/main", "main");
        state.record_success("aaa");
        let mut store = MockState::new();
        store
            .expect_load()
            .returning(move || Ok(Some(state.clone())));

        let mut evaluator = MockEvaluator::new();
        evaluator.expect_build_toplevel().times(0);

        let ctx = context(git, store, evaluator, MockDeployer::new());
        let report = SyncReconcilerUseCase::new(ctx)
            .reconcile_once(Path::new("/repo"), &options(), || async {
                Ok(vec![host()])
            })
            .await
            .unwrap();

        assert!(report
            .refused
            .as_deref()
            .unwrap_or_default()
            .contains("downgrade"));
    }

    #[tokio::test]
    async fn dry_run_reports_drift_without_activating() {
        let mut git = MockGit::new();
        git.expect_is_dirty().returning(|_| Ok(false));
        git.expect_local_head().returning(|_| Ok("bbb".into()));
        git.expect_remote_head()
            .returning(|_, _, _| Ok("bbb".into()));

        let mut state = SyncState::new("origin/main", "main");
        state.record_success("aaa");
        let mut store = MockState::new();
        store
            .expect_load()
            .returning(move || Ok(Some(state.clone())));

        let mut evaluator = MockEvaluator::new();
        evaluator
            .expect_build_toplevel()
            .returning(|_, _, _, _, _| Ok(PathBuf::from("/nix/store/new")));

        let mut deployer = MockDeployer::new();
        deployer.expect_check_reachability().returning(|_| Ok(true));
        deployer
            .expect_current_closure()
            .returning(|_, _| Ok(Some(PathBuf::from("/nix/store/old"))));

        let mut options = options();
        options.dry_run = true;

        let ctx = context(git, store, evaluator, deployer);
        let report = SyncReconcilerUseCase::new(ctx)
            .reconcile_once(Path::new("/repo"), &options, || async { Ok(vec![host()]) })
            .await
            .unwrap();

        assert!(!report.applied);
        assert_eq!(report.hosts.len(), 1);
        assert_eq!(report.hosts[0].result, SyncHostResult::Skipped);
        assert_eq!(report.plan.len(), 1, "a plan describes the drifted host");
        assert!(report.plan[0].current.is_some());
    }

    #[tokio::test]
    async fn a_preview_does_not_integrate_a_behind_checkout() {
        let mut git = MockGit::new();
        git.expect_is_dirty().returning(|_| Ok(false));
        git.expect_local_head().returning(|_| Ok("aaa".into()));
        git.expect_remote_head()
            .returning(|_, _, _| Ok("bbb".into()));
        // No fetch/ff expectation: a preview that integrates would panic here.

        let mut store = MockState::new();
        store.expect_load().returning(|| Ok(None));

        let mut evaluator = MockEvaluator::new();
        evaluator.expect_build_toplevel().times(0);

        let ctx = context(git, store, evaluator, MockDeployer::new());
        let mut options = options();
        options.dry_run = true;

        let report = SyncReconcilerUseCase::new(ctx)
            .reconcile_once(Path::new("/repo"), &options, || async { Ok(vec![host()]) })
            .await
            .unwrap();

        assert!(
            report
                .refused
                .as_deref()
                .unwrap_or_default()
                .contains("behind"),
            "a preview must say it cannot see the new revision: {report:?}"
        );
    }

    #[tokio::test]
    async fn an_unreachable_host_is_skipped_not_failed() {
        let mut git = MockGit::new();
        git.expect_is_dirty().returning(|_| Ok(false));
        git.expect_local_head().returning(|_| Ok("aaa".into()));
        git.expect_remote_head()
            .returning(|_, _, _| Ok("bbb".into()));
        git.expect_fetch().returning(|_, _, _| Ok(()));
        git.expect_is_ancestor().returning(|_, _, _| Ok(true));
        git.expect_fast_forward()
            .returning(|_, _, _| Ok("bbb".into()));

        let mut state = SyncState::new("origin/main", "main");
        state.record_success("aaa");
        let mut store = MockState::new();
        store
            .expect_load()
            .returning(move || Ok(Some(state.clone())));
        store.expect_save().returning(|_| Ok(()));

        let mut evaluator = MockEvaluator::new();
        evaluator.expect_build_toplevel().times(0);

        let mut deployer = MockDeployer::new();
        deployer
            .expect_check_reachability()
            .returning(|_| Ok(false));

        let ctx = context(git, store, evaluator, deployer);
        let report = SyncReconcilerUseCase::new(ctx)
            .reconcile_once(Path::new("/repo"), &options(), || async {
                Ok(vec![host()])
            })
            .await
            .unwrap();

        assert!(
            !report.applied,
            "an unobserved fleet is not a reconciled fleet"
        );
        assert!(
            report.error.is_none(),
            "an unreachable host is not a failure"
        );
        assert_eq!(report.hosts.len(), 1);
        assert_eq!(report.hosts[0].result, SyncHostResult::Unreachable);
    }

    #[tokio::test]
    async fn an_unreachable_cycle_records_the_revision_but_keeps_probing() {
        let mut git = MockGit::new();
        git.expect_is_dirty().returning(|_| Ok(false));
        git.expect_local_head().returning(|_| Ok("aaa".into()));
        git.expect_remote_head()
            .returning(|_, _, _| Ok("bbb".into()));
        git.expect_fetch().returning(|_, _, _| Ok(()));
        git.expect_is_ancestor().returning(|_, _, _| Ok(true));
        git.expect_fast_forward()
            .returning(|_, _, _| Ok("bbb".into()));

        let mut state = SyncState::new("origin/main", "main");
        state.record_success("aaa");
        let saved = std::sync::Arc::new(std::sync::Mutex::new(Vec::<SyncState>::new()));
        let saved_for_load = saved.clone();
        let mut store = MockState::new();
        store.expect_load().returning(move || {
            let current = saved_for_load.lock().unwrap().last().cloned();
            Ok(Some(current.unwrap_or_else(|| {
                let mut seed = SyncState::new("origin/main", "main");
                seed.record_success("aaa");
                seed
            })))
        });
        let saved_for_save = saved.clone();
        store.expect_save().returning(move |next| {
            saved_for_save.lock().unwrap().push(next.clone());
            Ok(())
        });

        let mut evaluator = MockEvaluator::new();
        evaluator.expect_build_toplevel().times(0);

        let mut deployer = MockDeployer::new();
        deployer
            .expect_check_reachability()
            .returning(|_| Ok(false));

        let ctx = context(git, store, evaluator, deployer);
        SyncReconcilerUseCase::new(ctx)
            .reconcile_once(Path::new("/repo"), &options(), || async {
                Ok(vec![host()])
            })
            .await
            .unwrap();

        let last = saved.lock().unwrap().last().cloned().unwrap();
        assert_eq!(
            last.last_applied_rev.as_deref(),
            Some("bbb"),
            "the observed fleet reached the revision"
        );
        assert!(
            matches!(
                last.host("atlas").map(|memory| &memory.condition),
                Some(HostCondition::Offline { .. })
            ),
            "the missing host is remembered as offline instead of forcing a full sweep"
        );
    }

    #[tokio::test]
    async fn probe_only_cycle_does_not_rebuild_when_hosts_stay_unreachable() {
        let mut git = MockGit::new();
        git.expect_is_dirty().returning(|_| Ok(false));
        git.expect_local_head().returning(|_| Ok("bbb".into()));
        git.expect_remote_head()
            .returning(|_, _, _| Ok("bbb".into()));

        let mut state = SyncState::new("origin/main", "main");
        state.record_success("bbb");
        state.observe_offline(
            "atlas",
            now_secs().saturating_sub(120),
            &RetryPolicy::from_options(&options()),
        );
        let mut store = MockState::new();
        store
            .expect_load()
            .returning(move || Ok(Some(state.clone())));
        store.expect_save().returning(|_| Ok(()));

        let mut evaluator = MockEvaluator::new();
        evaluator.expect_build_toplevel().times(0);

        let mut deployer = MockDeployer::new();
        deployer
            .expect_check_reachability()
            .times(1)
            .returning(|_| Ok(false));

        let ctx = context(git, store, evaluator, deployer);
        let report = SyncReconcilerUseCase::new(ctx)
            .reconcile_once(Path::new("/repo"), &options(), || async {
                Ok(vec![host()])
            })
            .await
            .unwrap();

        assert!(!report.applied, "nothing was observed, so nothing settled");
        assert_eq!(report.hosts.len(), 1);
        assert_eq!(report.hosts[0].result, SyncHostResult::Unreachable);
    }

    #[tokio::test]
    async fn a_returning_host_triggers_a_full_check() {
        let mut git = MockGit::new();
        git.expect_is_dirty().returning(|_| Ok(false));
        git.expect_local_head().returning(|_| Ok("bbb".into()));
        git.expect_remote_head()
            .returning(|_, _, _| Ok("bbb".into()));

        let mut state = SyncState::new("origin/main", "main");
        state.record_success("bbb");
        state.observe_offline(
            "atlas",
            now_secs().saturating_sub(120),
            &RetryPolicy::from_options(&options()),
        );
        let mut store = MockState::new();
        store
            .expect_load()
            .returning(move || Ok(Some(state.clone())));
        store.expect_save().returning(|_| Ok(()));

        let mut evaluator = MockEvaluator::new();
        evaluator
            .expect_build_toplevel()
            .times(1)
            .returning(|_, _, _, _, _| Ok(PathBuf::from("/nix/store/current")));

        let mut deployer = MockDeployer::new();
        deployer.expect_check_reachability().returning(|_| Ok(true));
        deployer
            .expect_current_closure()
            .returning(|_, _| Ok(Some(PathBuf::from("/nix/store/current"))));

        let ctx = context(git, store, evaluator, deployer);
        let report = SyncReconcilerUseCase::new(ctx)
            .reconcile_once(Path::new("/repo"), &options(), || async {
                Ok(vec![host()])
            })
            .await
            .unwrap();

        assert!(
            report.applied,
            "a reachable, current host settles the fleet"
        );
        assert_eq!(report.hosts[0].result, SyncHostResult::Unchanged);
    }

    #[test]
    fn unit_summary_keeps_lifecycle_lines_and_drops_noise() {
        let output = "Checking switch inhibitors... done\nstopping the following units: a.service\nstarting the following units: b.service\nunrelated chatter\n";
        let summary = SyncReconcilerUseCase::unit_summary(output).unwrap();
        assert!(summary.contains("a.service"), "got: {summary}");
        assert!(summary.contains("b.service"), "got: {summary}");
        assert!(!summary.contains("unrelated"), "got: {summary}");
    }

    #[test]
    fn unit_summary_is_none_without_lifecycle_lines() {
        assert!(SyncReconcilerUseCase::unit_summary("nothing to do here\n").is_none());
    }
}
