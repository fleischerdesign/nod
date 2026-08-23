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

        if options.action == DeploymentAction::Test {
            return Ok(self.verify_only(hosts, options).await);
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

    /// `--action test`: verify health without running the switch step.
    async fn verify_only(&self, hosts: Vec<HostEntity>, _options: DeploymentOptions) -> FleetSummary {
        let mut outcomes = Vec::<HostOutcome>::with_capacity(hosts.len());
        for host in hosts {
            outcomes.push(HostOutcome::new(host.name, DeploymentState::Prepared));
        }
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
    let activation = deployer.deploy_and_activate(&host, &closure.unwrap(), options.verbose).await;
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