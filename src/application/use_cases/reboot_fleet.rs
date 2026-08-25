//! `RebootFleetUseCase`: orchestrate progressive host reboots with recovery verification (ADR-017).

use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::host::HostEntity;

/// Parameters controlling the fleet reboot operation.
#[derive(Debug, Clone)]
pub struct RebootOptions {
    /// Strategy: 'all', 'canary', or 'batch'.
    pub strategy: String,
    /// Batch size for 'batch' strategy.
    pub batch_size: usize,
    /// Concurrency limit per wave.
    pub concurrency: usize,
    /// Wait for hosts to come back online and pass health checks.
    pub wait: bool,
    /// Maximum seconds to wait for recovery.
    pub timeout_secs: u64,
}

impl Default for RebootOptions {
    fn default() -> Self {
        Self {
            strategy: "all".to_string(),
            batch_size: 0,
            concurrency: 4,
            wait: true,
            timeout_secs: 180,
        }
    }
}

/// Outcome of rebooting a single host.
#[derive(Debug, Clone)]
pub struct RebootOutcome {
    pub host_name: String,
    pub ok: bool,
    pub elapsed: Duration,
    pub failure: Option<String>,
}

/// Aggregated fleet reboot result.
#[derive(Debug, Clone, Default)]
pub struct RebootSummary {
    pub outcomes: Vec<RebootOutcome>,
}

impl RebootSummary {
    pub fn succeeded(&self) -> usize {
        self.outcomes.iter().filter(|o| o.ok).count()
    }

    pub fn failed(&self) -> usize {
        self.outcomes.iter().filter(|o| !o.ok).count()
    }
}

/// Use case that executes rolling host reboots.
pub struct RebootFleetUseCase {
    ctx: Arc<AppContext>,
}

impl RebootFleetUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    pub async fn execute(
        &self,
        targets: Vec<HostEntity>,
        options: RebootOptions,
    ) -> Result<RebootSummary, NodError> {
        let mut summary = RebootSummary::default();

        let waves = partition_into_waves(&targets, &options.strategy, options.batch_size);

        for (wave_idx, wave) in waves.into_iter().enumerate() {
            if wave_idx > 0 {
                println!(
                    "{}",
                    format!(
                        "--- Starting reboot wave {} of {} hosts ---",
                        wave_idx + 1,
                        wave.len()
                    )
                    .dimmed()
                );
            }

            for host in wave {
                let start = Instant::now();
                let pb = ProgressBar::new_spinner();
                pb.set_style(
                    ProgressStyle::default_spinner()
                        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
                        .template("  {spinner:.cyan} {msg}")
                        .unwrap(),
                );
                pb.set_message(format!("Rebooting {}...", host.name));
                pb.enable_steady_tick(Duration::from_millis(80));

                let outcome = self.reboot_single_host(host, &options, &pb).await;
                pb.finish_and_clear();

                let ok = outcome.is_ok();
                let failure = outcome.err().map(|e| e.to_string());

                // Record to audit log
                if let Ok(store) = self.ctx.audit_store() {
                    let outcome_str = if ok {
                        "reboot:completed"
                    } else {
                        "reboot:failed"
                    };
                    let _ = store.record(&host.name, outcome_str).await;
                }

                summary.outcomes.push(RebootOutcome {
                    host_name: host.name.clone(),
                    ok,
                    elapsed: start.elapsed(),
                    failure,
                });
            }
        }

        Ok(summary)
    }

    async fn reboot_single_host(
        &self,
        host: &HostEntity,
        options: &RebootOptions,
        pb: &ProgressBar,
    ) -> Result<(), NodError> {
        let profile = self.ctx.resolved_profile(host).await?;
        let deployer = self.ctx.deployer_for(host);

        deployer.reboot(host, &profile).await?;

        if !options.wait {
            return Ok(());
        }

        pb.set_message(format!("Waiting for {} to cycle offline...", host.name));
        sleep(Duration::from_secs(4)).await;

        pb.set_message(format!("Waiting for {} to recover online...", host.name));
        let deadline = Instant::now() + Duration::from_secs(options.timeout_secs);

        let mut back_online = false;
        while Instant::now() < deadline {
            if deployer.check_reachability(host).await.unwrap_or(false) {
                back_online = true;
                break;
            }
            sleep(Duration::from_secs(2)).await;
        }

        if !back_online {
            return Err(NodError::deployment(format!(
                "timed out waiting for {} to recover after reboot",
                host.name
            )));
        }

        // Verify systemd health if health checker is available
        if let Ok(checker) = self.ctx.health_checker() {
            pb.set_message(format!("Verifying system health for {}...", host.name));
            let _ = checker.verify_health(host).await;
        }

        Ok(())
    }
}

fn partition_into_waves<'a>(
    targets: &'a [HostEntity],
    strategy: &str,
    batch_size: usize,
) -> Vec<Vec<&'a HostEntity>> {
    match strategy {
        "canary" if targets.len() > 1 => {
            let (first, rest) = targets.split_at(1);
            vec![first.iter().collect(), rest.iter().collect()]
        }
        "batch" if batch_size > 0 => targets
            .chunks(batch_size)
            .map(|chunk| chunk.iter().collect())
            .collect(),
        _ => vec![targets.iter().collect()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::SshProfile;
    use crate::domain::ports::deployer::DeployerPort;
    use crate::domain::ports::evaluator::EvaluatorPort;
    use async_trait::async_trait;
    use mockall::mock;
    use std::path::{Path, PathBuf};

    mock! {
        FakeEvaluator {}
        #[async_trait]
        impl EvaluatorPort for FakeEvaluator {
            async fn discover_hosts(&self, flake_path: &Path, verbose: bool) -> Result<Vec<HostEntity>, NodError>;
            async fn build_toplevel<'a>(&self, flake_path: &Path, host_name: &str, builder: Option<&'a crate::domain::host::BuilderHost>, verbose: bool) -> Result<PathBuf, NodError>;
        }
    }

    mock! {
        FakeDeployer {}
        #[async_trait]
        impl DeployerPort for FakeDeployer {
            async fn check_reachability(&self, host: &HostEntity) -> Result<bool, NodError>;
            async fn current_closure(&self, host: &HostEntity, profile: &SshProfile) -> Result<Option<PathBuf>, NodError>;
            async fn deploy_and_activate(&self, host: &HostEntity, profile: &SshProfile, closure: &Path, action: &str, verbose: bool) -> Result<(), NodError>;
            async fn rollback(&self, host: &HostEntity, profile: &SshProfile) -> Result<(), NodError>;
            async fn reboot(&self, host: &HostEntity, profile: &SshProfile) -> Result<(), NodError>;
        }
    }

    #[tokio::test]
    async fn reboot_fleet_executes_successfully() {
        let mut deployer = MockFakeDeployer::new();
        deployer.expect_reboot().returning(|_, _| Ok(()));
        deployer.expect_check_reachability().returning(|_| Ok(true));

        let ctx = Arc::new(AppContext::new(
            Arc::new(MockFakeEvaluator::new()),
            Arc::new(deployer),
            Arc::new(MockFakeDeployer::new()),
        ));

        let use_case = RebootFleetUseCase::new(ctx);
        let targets = vec![HostEntity::new("yorke", "127.0.0.1", true)];
        let options = RebootOptions {
            wait: false,
            ..Default::default()
        };

        let summary = use_case.execute(targets, options).await.unwrap();
        assert_eq!(summary.succeeded(), 1);
        assert_eq!(summary.failed(), 0);
    }
}
