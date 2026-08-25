//! `nod eval` command: evaluate Nix expressions in host context across the fleet (ADR-016).

use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::selection::{resolve_targets, DefaultScope, TargetSelection};
use crate::application::use_cases::eval_fleet::EvalFleetUseCase;
use crate::config::options::TargetArgs;
use crate::domain::errors::NodError;

pub async fn execute(
    ctx: AppContext,
    flake_path: &Path,
    target_args: &TargetArgs,
    expr: &str,
    raw: bool,
    verbose: bool,
    json: bool,
) -> Result<(), NodError> {
    let evaluator = ctx.evaluator();
    let hosts = evaluator
        .discover_hosts_degraded(flake_path, verbose)
        .await?;
    let local_hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();

    let targets = resolve_targets(
        hosts,
        &local_hostname,
        target_args.target.as_deref(),
        target_args.tag.as_deref(),
        target_args.role.as_deref(),
        target_args.all,
        DefaultScope::Local,
    );

    if targets.is_empty() {
        return Err(TargetSelection::unmatched(
            target_args.target.as_deref().unwrap_or("local"),
            target_args.tag.as_deref(),
            target_args.role.as_deref(),
        ));
    }

    let use_case = EvalFleetUseCase::new(Arc::new(ctx));
    let results = use_case.execute(&targets, flake_path, expr, json).await?;

    if json {
        println!("{}", serde_json::to_string(&results).unwrap());
        return Ok(());
    }

    for res in results {
        if targets.len() > 1 {
            print!("{}: ", res.host_name.bold());
        }

        let output = if raw {
            res.raw_output.trim_matches('"')
        } else {
            &res.raw_output
        };

        println!("{}", output.cyan());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::{HostEntity, SshProfile};
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
    async fn eval_command_runs_successfully() {
        let mut eval = MockFakeEvaluator::new();
        eval.expect_discover_hosts()
            .returning(|_, _| Ok(vec![HostEntity::new("yorke", "127.0.0.1", true)]));
        eval.expect_eval_expr()
            .returning(|_, _, _, _| Ok("\"yorke\"".to_string()));

        let ctx = AppContext::new(
            Arc::new(eval),
            Arc::new(MockFakeDeployer::new()),
            Arc::new(MockFakeDeployer::new()),
        );

        let res = execute(
            ctx,
            Path::new("."),
            &TargetArgs::default(),
            "networking.hostName",
            false,
            false,
            false,
        )
        .await;
        assert!(res.is_ok());
    }
}
