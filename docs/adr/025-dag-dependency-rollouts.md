# ADR-025: DAG Dependency-based Wave Deployments

## Context

In multi-tier infrastructure environments (e.g. database nodes, backend API services, reverse proxies/ingresses), hosts have logical deployment order dependencies. Deploying an ingress proxy before the backend APIs are active or deploying backend services before database migrations complete can lead to service downtime.

Previously, `nod` supported numerical priority levels (`config.nod.rollout.priority`) and fixed batch sizes, but did not support declarative inter-host dependency graphs (`dependsOn`) with automatic topological ordering and cycle detection.

## Decision

1. **Declarative Host Dependencies**:
   - Introduce `dependsOn` / `depends_on: Vec<String>` in:
     - The NixOS module (`options.nod.dependsOn` and `options.nod.rollout.dependsOn`).
     - Configuration structs (`NodConfig` and `RolloutConfig`).
     - Domain target representations (`TargetPlan.depends_on`).

2. **Topological Level Wave Partitioning (Kahn's Algorithm)**:
   - `DeploymentPlan::wave_indices` calculates a Directed Acyclic Graph (DAG) across targeted hosts:
     - Nodes with in-degree 0 (no unresolved dependencies) form the current execution wave level.
     - Upon completion of a level, dependent nodes are unblocked.
     - Cycle detection: If uncompleted targets remain with unresolved dependencies, `NodError::config` is returned describing the cyclic dependency.
   - Within each topological level, targets are rolled out respecting `--concurrency` and `--strategy` (`All`, `Batch`, `Canary`).

3. **Fail-Fast Boundary**:
   - When a host in an earlier dependency level fails deployment or health verification, execution terminates (when `--fail-fast` is set), protecting downstream dependent hosts from running on an unhealthy upstream.

## Consequences

- **Safety**: Safe orchestration of multi-tier stacks with declarative dependencies.
- **Cycle Prevention**: Erroneous dependency loops are caught before any deployment action is initiated.
- **Backward Compatibility**: Fleets without explicit `dependsOn` declarations continue to partition into standard waves with zero behavior change.
