//! `DeployFleetUseCase`: rollout the fleet under the ADR-005 concurrency
//! policy, one ADR-003 state machine per host (ADR-003).
//!
//! Each host acquires a semaphore permit (bounding in-flight hosts to
//! `--concurrency N`), drives its lifecycle, and reports a `HostOutcome`.
//! Rollout strategy and error mode are pure decisions in `domain/plan.rs` +
//! this module; the port calls remain mockable for tests.

use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::application::context::AppContext;
use crate::application::pipeline::state_machine::{
    DeploymentEvent, DeploymentState, DeploymentStateMachine,
};
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;
use crate::domain::plan::{DeploymentAction, DeploymentOptions, DeploymentPlan, TargetPlan};

/// The per-host result of a deployment attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostOutcome {
    /// The host that was targeted.
    pub host_name: String,
    /// The terminal lifecycle state reached.
    pub state: DeploymentState,
    /// True when the host reached a good terminal state (`Completed` or a dry-
    /// run staged/preview result).
    pub ok: bool,
    /// True when the host was recovered via a rollback.
    pub rolled_back: bool,
}

impl HostOutcome {
    /// Builds an outcome for `state`, marking `ok` for good terminal states.
    pub fn new(host_name: impl Into<String>, state: DeploymentState) -> Self {
        let ok = state == DeploymentState::Completed || state == DeploymentState::Prepared;
        Self {
            host_name: host_name.into(),
            state: state.clone(),
            ok,
            rolled_back: state == DeploymentState::RolledBack,
        }
    }
}

/// Aggregated results of a fleet rollout.
#[derive(Debug, Clone)]
pub struct FleetSummary {
    /// Per-host outcomes in wave order.
    pub outcomes: Vec<HostOutcome>,
    /// True when a `--fail-fast` run aborted before other waves completed.
    pub aborted: bool,
}

impl FleetSummary {
    /// Number of hosts that reached `Completed`.
    pub fn succeeded(&self) -> usize {
        self.outcomes.iter().filter(|o| o.state == DeploymentState::Completed).count()
    }

    /// Number of hosts recovered through `RolledBack`.
    pub fn rolled_back(&self) -> usize {
        self.outcomes.iter().filter(|o| o.state == DeploymentState::RolledBack).count()
    }

    /// Number of hosts that ended `Failed`.
    pub fn failed(&self) -> usize {
        self.outcomes.iter().filter(|o| o.state == DeploymentState::Failed).count()
    }
}

/// Fleet deployment policy executor (ADR-005).
pub struct DeployFleetUseCase {
    ctx: Arc<AppContext>,
}

impl DeployFleetUseCase {
    /// Builds the use case over a seeded context.
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    /// Runs `options` over `hosts`, respecting concurrency and rollout.
    pub async fn execute(
        &self,
        hosts: Vec<HostEntity>,
        options: DeploymentOptions,
        flake_path: &Path,
    ) -> Result<FleetSummary, NodError> {
        if hosts.is_empty() {
            return Err(NodError::config("no hosts targeted for deployment"));
        }

        if options.is_dry_run() {
            return Ok(self.stage_preview(hosts, options).await);
        }

        if options.concurrency == 0 {
            return Err(NodError::config("--concurrency must be at least 1"));
        }

        if options.action == DeploymentAction::Build {
            return Ok(self.build_only(hosts, options, flake_path).await);
        }

        let plan = self.plan_for(hosts.clone(), options.clone());
        let sem = Arc::new(Semaphore::new(options.concurrency));
        let mut summary = FleetSummary { outcomes: Vec::new(), aborted: false };

        for wave in plan.wave_indices() {
            let wave_outcomes = self.run_wave(&hosts, &wave, options.clone(), sem.clone(), flake_path).await;
            let mut failed = false;
            for outcome in wave_outcomes {
                if !outcome.ok {
                    failed = true;
                }
                summary.outcomes.push(outcome);
            }
            if failed && options.fail_fast {
                summary.aborted = true;
                break;
            }
        }
        Ok(summary)
    }

    /// Runs one rollout wave; each host drives a state machine concurrently,
    /// with in-flight concurrency bounded by the semaphore.
    async fn run_wave(
        &self,
        hosts: &Vec<HostEntity>,
        wave: &Vec<usize>,
        options: DeploymentOptions,
        sem: Arc<Semaphore>,
        flake_path: &Path,
    ) -> Vec<HostOutcome> {
        let flake = flake_path.display().to_string();
        let mut set = JoinSet::<HostOutcome>::new();
        let mut k = 0;
        while k < (*wave).len() {
            let idx = (*wave)[k];
            k += 1;
            let host_c = (*hosts)[idx].clone();
            let opts_c = options.clone();
            let sem_c = sem.clone();
            let ctx_c = self.ctx.clone();
            let flake_c = flake.clone();
            set.spawn(async move {
                run_host(ctx_c, host_c, opts_c, sem_c, flake_c).await
            });
        }
        set.join_all().await
    }

    /// `--action build`: evaluate and build each host's toplevel closure,
    /// creating the out-link symlink when requested, and never transferring or
    /// activating (ADR-006 lifecycle commands). Builds run concurrently under
    /// the same semaphore budget as activation waves (ADR-005).
    async fn build_only(
        &self,
        hosts: Vec<HostEntity>,
        options: DeploymentOptions,
        flake_path: &Path,
    ) -> FleetSummary {
        let sem = Arc::new(Semaphore::new(options.concurrency));
        let mut set = JoinSet::<HostOutcome>::new();
        let flake = flake_path.display().to_string();
        let out_link = options.out_link.clone();
        let verbose = options.verbose;
        let evaluator = self.ctx.evaluator();

        for host in hosts {
            let sem_c = sem.clone();
            let evaluator_c = evaluator.clone();
            let flake_c = flake.clone();
            let out_link_c = out_link.clone();
            let name_c = host.name.clone();
            set.spawn(async move {
                let _permit = sem_c.acquire().await.ok();
                let built = evaluator_c
                    .build_toplevel(Path::new(&flake_c), &name_c, verbose)
                    .await;
                match built {
                    Ok(closure) => {
                        if let Some(link) = out_link_c {
                            // Best-effort: a symlink failure does not fail
                            // the build (the closure was already built).
                            std::os::unix::fs::symlink(&closure, &link).ok();
                        }
                        HostOutcome::new(name_c, DeploymentState::Prepared)
                    }
                    Err(_) => HostOutcome::new(name_c, DeploymentState::Failed),
                }
            });
        }

        let outcomes = set.join_all().await;
        FleetSummary { outcomes, aborted: false }
    }

    /// Dry-run: stage every host as `Prepared`; the closures were built but no
    /// switch was ever issued.
    async fn stage_preview(&self, hosts: Vec<HostEntity>, options: DeploymentOptions) -> FleetSummary {
        let plan = self.plan_for(hosts, options);
        let mut outcomes = Vec::<HostOutcome>::with_capacity(plan.targets.len());
        for target in plan.targets {
            outcomes.push(HostOutcome::new(target.host_name, DeploymentState::Prepared));
        }
        FleetSummary { outcomes, aborted: false }
    }

    /// Builds a `DeploymentPlan` (ordered target list + policy).
    pub fn plan_for(&self, hosts: Vec<HostEntity>, options: DeploymentOptions) -> DeploymentPlan {
        let mut targets = Vec::<TargetPlan>::with_capacity(hosts.len());
        for host in hosts {
            targets.push(TargetPlan {
                host_name: host.name,
                action: options.action.clone(),
                new_closure: None,
                current_closure: None,
            });
        }
        DeploymentPlan { targets, options }
    }
}

/// Drives one host through its ADR-003 lifecycle under a semaphore permit.
async fn run_host(
    ctx: Arc<AppContext>,
    host: HostEntity,
    options: DeploymentOptions,
    sem: Arc<Semaphore>,
    flake: String,
) -> HostOutcome {
    let acquired = sem.acquire().await;
    if acquired.is_err() {
        return HostOutcome::new(host.name, DeploymentState::Failed);
    }
    // Permit stays alive for the whole host; dropping it frees the slot.
    let _ = acquired.unwrap();

    let mut machine = DeploymentStateMachine::prepared();
    machine.tick(DeploymentEvent::Begin).unwrap();

    let evaluator = ctx.evaluator();
    let closure = evaluator
        .build_toplevel(std::path::Path::new(&flake), &host.name, options.verbose)
        .await;

    if closure.is_err() {
        return end_host(&mut machine, DeploymentEvent::EvalFail, &host, &options, &ctx).await;
    }

    machine.tick(DeploymentEvent::EvalOk).unwrap();
    machine.tick(DeploymentEvent::BuildOk).unwrap();
    machine.tick(DeploymentEvent::TransferOk).unwrap();

    let deployer = ctx.deployer_for(&host);
    let activation_action = options.action.to_str();
    let activation = deployer
        .deploy_and_activate(&host, &closure.unwrap(), &activation_action, options.verbose)
        .await;
    if activation.is_err() {
        return end_host(&mut machine, DeploymentEvent::SwitchFail, &host, &options, &ctx).await;
    }

    machine.tick(DeploymentEvent::SwitchOk).unwrap();

    if let Some(health) = ctx.health_checker_opt() {
        let verified = health.verify_health(&host).await;
        if verified.is_err() || !verified.unwrap() {
            return end_host(&mut machine, DeploymentEvent::VerifyFail, &host, &options, &ctx).await;
        }
    }

    machine.tick(DeploymentEvent::VerifyOk).unwrap();
    HostOutcome::new(host.name, machine.state())
}

/// Advances to `RollbackTriggered`, then rolls back when enabled (ADR-003).
async fn end_host(
    machine: &mut DeploymentStateMachine,
    event: DeploymentEvent,
    host: &HostEntity,
    options: &DeploymentOptions,
    ctx: &Arc<AppContext>,
) -> HostOutcome {
    machine.tick(event).unwrap();
    let deployer = ctx.deployer_for(host);
    if options.auto_rollback {
        let rollback = deployer.rollback(host).await;
        if rollback.is_ok() {
            machine.tick(DeploymentEvent::RollbackOk).unwrap();
            return HostOutcome::new(host.name.clone(), machine.state());
        }
    }
    machine.tick(DeploymentEvent::RollbackFail).unwrap();
    HostOutcome::new(host.name.clone(), machine.state())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use async_trait::async_trait;
    use mockall::mock;
    use std::path::PathBuf;

    mock! {
        FakeDeployer {}
        #[async_trait]
        impl DeployerPort for FakeDeployer {
            async fn check_reachability(&self, host: &HostEntity) -> Result<bool, NodError>;
            async fn current_closure(&self, host: &HostEntity) -> Result<Option<PathBuf>, NodError>;
            async fn deploy_and_activate(&self, host: &HostEntity, closure: &Path, action: &str, verbose: bool) -> Result<(), NodError>;
            async fn rollback(&self, host: &HostEntity) -> Result<(), NodError>;
        }
    }

    mock! {
        FakeEvaluator {}
        #[async_trait]
        impl EvaluatorPort for FakeEvaluator {
            async fn discover_hosts(&self, flake_path: &Path, verbose: bool) -> Result<Vec<HostEntity>, NodError>;
            async fn build_toplevel(&self, flake_path: &Path, host_name: &str, verbose: bool) -> Result<PathBuf, NodError>;
        }
    }

    /// Context with the evaluator and both deployer slots bound to mocks.
    fn ctx_with(
        eval: MockFakeEvaluator,
        local: MockFakeDeployer,
        ssh: MockFakeDeployer,
    ) -> Arc<AppContext> {
        Arc::new(AppContext::new(
            Arc::new(eval),
            Arc::new(local),
            Arc::new(ssh),
        ))
    }

    fn options_with(action: DeploymentAction) -> DeploymentOptions {
        let mut options = DeploymentOptions::default_policy();
        options.action = action;
        options.concurrency = 1;
        options.fail_fast = false;
        options.auto_rollback = false;
        options
    }

    #[tokio::test]
    async fn test_action_runs_switch_to_configuration_test() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_build_toplevel()
            .times(1)
            .returning(|_, _, _| Ok(PathBuf::from("/nix/store/aaa-test")));

        let mut local = MockFakeDeployer::new();
        local
            .expect_deploy_and_activate()
            .times(1)
            .withf(|_, _, action, _| action == "test")
            .returning(|_, _, _, _| Ok(()));

        let ctx = ctx_with(eval, local, MockFakeDeployer::new());
        let host = HostEntity::new("jello", "jello-machine", true);
        let use_case = DeployFleetUseCase::new(ctx);

        let summary = use_case
            .execute(vec![host], options_with(DeploymentAction::Test), Path::new("/tmp/flake"))
            .await
            .unwrap();

        assert_eq!(summary.outcomes.len(), 1);
        assert_eq!(summary.outcomes[0].state, DeploymentState::Completed);
        assert!(summary.outcomes[0].ok);
        assert!(!summary.aborted);
    }

    #[tokio::test]
    async fn boot_action_runs_switch_to_configuration_boot() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_build_toplevel()
            .times(1)
            .returning(|_, _, _| Ok(PathBuf::from("/nix/store/aaa-boot")));

        let mut local = MockFakeDeployer::new();
        local
            .expect_deploy_and_activate()
            .times(1)
            .withf(|_, _, action, _| action == "boot")
            .returning(|_, _, _, _| Ok(()));

        let ctx = ctx_with(eval, local, MockFakeDeployer::new());
        let host = HostEntity::new("jello", "jello-machine", true);
        let use_case = DeployFleetUseCase::new(ctx);

        let summary = use_case
            .execute(vec![host], options_with(DeploymentAction::Boot), Path::new("/tmp/flake"))
            .await
            .unwrap();

        assert_eq!(summary.outcomes.len(), 1);
        assert_eq!(summary.outcomes[0].state, DeploymentState::Completed);
        assert!(summary.outcomes[0].ok);
    }

    #[tokio::test]
    async fn build_action_builds_without_transferring() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_build_toplevel()
            .times(1)
            .returning(|_, _, _| Ok(PathBuf::from("/nix/store/aaa-build")));

        // The build action must never reach a deployer (no transfer, no
        // activation, no rollback).
        let mut local = MockFakeDeployer::new();
        local.expect_deploy_and_activate().times(0);
        local.expect_rollback().times(0);

        let ctx = ctx_with(eval, local, MockFakeDeployer::new());
        let host = HostEntity::new("jello", "jello-machine", true);
        let use_case = DeployFleetUseCase::new(ctx);

        let summary = use_case
            .execute(vec![host], options_with(DeploymentAction::Build), Path::new("/tmp/flake"))
            .await
            .unwrap();

        assert_eq!(summary.outcomes.len(), 1);
        assert_eq!(summary.outcomes[0].state, DeploymentState::Prepared);
        assert!(summary.outcomes[0].ok);
    }

    #[tokio::test]
    async fn build_action_creates_the_out_link_when_requested() {
        let dir = tempfile::tempdir().unwrap();
        let closure = dir.path().join("closure");
        std::fs::write(&closure, b"toplevel").unwrap();
        let link = dir.path().join("result");

        let mut eval = MockFakeEvaluator::new();
        let closure_c = closure.clone();
        eval.expect_build_toplevel()
            .times(1)
            .returning(move |_, _, _| Ok(closure_c.clone()));

        let local = MockFakeDeployer::new();
        let ctx = ctx_with(eval, local, MockFakeDeployer::new());
        let host = HostEntity::new("jello", "jello-machine", true);
        let mut options = options_with(DeploymentAction::Build);
        options.out_link = Some(link.clone());

        let use_case = DeployFleetUseCase::new(ctx);
        let summary = use_case
            .execute(vec![host], options, Path::new("/tmp/flake"))
            .await
            .unwrap();

        assert_eq!(summary.outcomes.len(), 1);
        assert!(summary.outcomes[0].ok);
        assert!(link.exists(), "out-link symlink must be created");
    }

    #[tokio::test]
    async fn build_failure_marks_the_host_failed() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_build_toplevel()
            .times(1)
            .returning(|_, _, _| Err(NodError::build_failure("jello", "eval failed")));

        let local = MockFakeDeployer::new();
        let ctx = ctx_with(eval, local, MockFakeDeployer::new());
        let host = HostEntity::new("jello", "jello-machine", true);
        let use_case = DeployFleetUseCase::new(ctx);

        let summary = use_case
            .execute(vec![host], options_with(DeploymentAction::Build), Path::new("/tmp/flake"))
            .await
            .unwrap();

        assert_eq!(summary.outcomes.len(), 1);
        assert_eq!(summary.outcomes[0].state, DeploymentState::Failed);
        assert!(!summary.outcomes[0].ok);
    }
}
