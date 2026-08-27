# Specification: DAG Dependency-based Wave Deployments

## Purpose
Support declarative deployment dependencies between fleet hosts, generating topologically sorted execution waves and preventing cascading failures.

## Requirements & Acceptance Criteria

### 1. Configuration Surface
- **AC1**: `options.nod.dependsOn` and `options.nod.rollout.dependsOn` in `modules/nixos/default.nix` accept a list of host names.
- **AC2**: `NodConfig` and `RolloutConfig` in `src/domain/config.rs` deserialize `dependsOn` (and `depends_on`).
- **AC3**: `TargetPlan` in `src/domain/plan.rs` carries `depends_on: Vec<String>`.

### 2. Topological Wave Partitioning (DAG)
- **AC4**: `DeploymentPlan::wave_indices` partitions targets into waves such that any host depending on host $H$ is scheduled in a strictly later wave than $H$.
- **AC5**: Independent hosts in the same DAG level are grouped for parallel execution bounded by `--concurrency` and `--strategy`.
- **AC6**: Cyclic dependencies MUST be detected and return `NodError::Config` detailing the cycle without executing any deployments.
