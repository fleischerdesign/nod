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
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::plan::{DeploymentAction, DeploymentOptions, RolloutStrategy};
use crate::domain::sync::{now_secs, SyncHostReport, SyncHostResult, SyncReport, SyncState};
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
        F: FnOnce() -> Fut,
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
                let backoff = state.record_failure(now);
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
        if revision_unchanged && proven_applied && !recovered {
            let report = SyncReport {
                revision: local_before.clone(),
                previous_revision: Some(local_before),
                applied: true,
                hosts: Vec::new(),
                refused: None,
                error: None,
                backoff_secs: 0,
            };
            state.record_success(report.revision.clone());
            state_store.save(&state).await?;
            self.notify(&report, options).await;
            return Ok(report);
        }

        if revision_unchanged {
            // Recovered cycle or first observation: verify drifts below.
        } else {
            if let Err(error) = git
                .fetch(flake_path, &options.remote, &options.branch)
                .await
            {
                let backoff = state.record_failure(now);
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

        let hosts = select_hosts().await?;
        if hosts.is_empty() {
            return Err(NodError::config(
                "no hosts selected; pass --target/--all or set [sync].targets",
            ));
        }

        let (mut changed, mut reports) = self.drift(flake_path, hosts, options.verbose).await?;
        changed = Self::order_last(changed, &options.last_hosts);

        if options.dry_run {
            for host in &changed {
                reports.push(SyncHostReport::new(
                    host.name.clone(),
                    SyncHostResult::Skipped,
                ));
            }
            let report = SyncReport {
                revision: head.clone(),
                previous_revision: Some(local_before.clone()),
                applied: false,
                hosts: reports,
                refused: None,
                error: None,
                backoff_secs: 0,
            };
            self.notify(&report, options).await;
            return Ok(report);
        }

        if changed.is_empty() {
            let report = SyncReport {
                revision: head.clone(),
                previous_revision: Some(local_before),
                applied: true,
                hosts: reports,
                refused: None,
                error: None,
                backoff_secs: 0,
            };
            state.record_success(head);
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
            if !mapped.result.is_settled() {
                failed = true;
            }
            reports.push(mapped);
        }

        let applied = !failed;
        let mut report = SyncReport {
            revision: head.clone(),
            previous_revision: Some(local_before),
            applied,
            hosts: reports,
            refused: None,
            error: None,
            backoff_secs: 0,
        };

        if applied {
            state.record_success(head);
        } else {
            report.backoff_secs = state.record_failure(now_secs());
            report.error = Some(format!(
                "{} host(s) did not reach a settled state",
                report
                    .hosts
                    .iter()
                    .filter(|h| !h.result.is_settled())
                    .count()
            ));
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
    /// A build failure aborts the cycle: partial drift information would make
    /// the report lie about what the fleet runs.
    async fn drift(
        &self,
        flake_path: &Path,
        hosts: Vec<HostEntity>,
        verbose: bool,
    ) -> Result<(Vec<HostEntity>, Vec<SyncHostReport>), NodError> {
        let evaluator = self.ctx.evaluator();
        let mut changed = Vec::new();
        let mut reports = Vec::new();

        for host in hosts {
            // An unreachable host is not a failed host: it is skipped and
            // retried next cycle. Probing before building keeps an offline
            // laptop from turning every cycle into a failed deployment.
            let deployer = self.ctx.deployer_for(&host);
            match deployer.check_reachability(&host).await {
                Ok(true) => {}
                Ok(false) => {
                    reports.push(SyncHostReport::new(host.name, SyncHostResult::Unreachable));
                    continue;
                }
                Err(error) => {
                    reports.push(
                        SyncHostReport::new(host.name, SyncHostResult::Unreachable)
                            .with_detail(error.to_string()),
                    );
                    continue;
                }
            }

            let built = evaluator
                .build_toplevel(
                    flake_path,
                    &host.name,
                    host.closure_attr.clone(),
                    None,
                    verbose,
                )
                .await?;
            let profile = self.ctx.resolved_profile(&host).await?;
            let current = deployer.current_closure(&host, &profile).await?;

            if current.as_deref() == Some(built.as_path()) {
                reports.push(SyncHostReport::new(host.name, SyncHostResult::Unchanged));
            } else {
                changed.push(host);
            }
        }

        Ok((changed, reports))
    }

    /// Orders hosts so infrastructure the reconciler needs is deployed last.
    /// Hosts in `last_hosts` and the executing host declare a dependency on
    /// every other target, which the existing DAG planner turns into a final
    /// wave.
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
    fn map_outcome(outcome: &HostOutcome) -> SyncHostReport {
        let result = match outcome.state {
            DeploymentState::Completed => SyncHostResult::Deployed,
            DeploymentState::RolledBack => SyncHostResult::RolledBack,
            DeploymentState::Failed => SyncHostResult::Failed,
            _ => SyncHostResult::Skipped,
        };
        let mut report = SyncHostReport::new(outcome.host_name.clone(), result);
        if let Some(failure) = &outcome.failure {
            report = report.with_detail(failure.clone());
        }
        report
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
            async fn deploy_and_activate(&self, host: &HostEntity, profile: &SshProfile, closure: &Path, action: &str, verbose: bool) -> Result<(), NodError>;
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
            .returning(|_, _, _, _, _| Ok(()));

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
            report.applied,
            "an unreachable host must not fail the whole cycle"
        );
        assert_eq!(report.hosts.len(), 1);
        assert_eq!(report.hosts[0].result, SyncHostResult::Unreachable);
    }
}
