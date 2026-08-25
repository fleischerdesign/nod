//! `EvalFleetUseCase`: evaluate a Nix expression across target hosts (ADR-016).

use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::domain::eval::EvalResult;
use crate::domain::host::HostEntity;

/// Use case that evaluates a Nix expression across fleet hosts.
pub struct EvalFleetUseCase {
    ctx: Arc<AppContext>,
}

impl EvalFleetUseCase {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    pub async fn execute(
        &self,
        targets: &[HostEntity],
        flake_path: &Path,
        expr: &str,
        json: bool,
    ) -> Result<Vec<EvalResult>, NodError> {
        let evaluator = self.ctx.evaluator();
        let mut results = Vec::with_capacity(targets.len());

        for host in targets {
            let raw_output = evaluator
                .eval_expr(flake_path, &host.name, expr, json)
                .await?;

            let json_value = if json {
                serde_json::from_str(&raw_output).ok()
            } else {
                None
            };

            results.push(EvalResult {
                host_name: host.name.clone(),
                expression: expr.to_string(),
                raw_output,
                json_value,
            });
        }

        Ok(results)
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
    use std::path::PathBuf;

    mock! {
        FakeEvaluator {}
        #[async_trait]
        impl EvaluatorPort for FakeEvaluator {
            async fn discover_hosts(&self, flake_path: &Path, verbose: bool) -> Result<Vec<HostEntity>, NodError>;
            async fn build_toplevel<'a>(&self, flake_path: &Path, host_name: &str, builder: Option<&'a crate::domain::host::BuilderHost>, verbose: bool) -> Result<PathBuf, NodError>;
            async fn eval_expr(&self, flake_path: &Path, host_name: &str, expr: &str, json: bool) -> Result<String, NodError>;
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
        }
    }

    #[tokio::test]
    async fn eval_fleet_evaluates_across_targets() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_eval_expr()
            .returning(|_, host_name, expr, _| Ok(format!("\"{host_name}-{expr}\"")));

        let ctx = Arc::new(AppContext::new(
            Arc::new(eval),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        ));

        let use_case = EvalFleetUseCase::new(ctx);
        let targets = vec![
            HostEntity::new("yorke", "127.0.0.1", true),
            HostEntity::new("rollins", "100.126.5.72", false),
        ];

        let results = use_case
            .execute(&targets, Path::new("."), "networking.hostName", false)
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].raw_output, "\"yorke-networking.hostName\"");
        assert_eq!(results[1].raw_output, "\"rollins-networking.hostName\"");
    }
}
