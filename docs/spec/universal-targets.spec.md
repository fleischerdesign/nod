# Specification: Universal Target Activation Contract

## Purpose
Enable `nod` to discover, evaluate, orchestrate, and activate heterogeneous and agentless infrastructure targets declared via the `nodTargets` flake output, establishing universal target polymorphism without hardcoded domain knowledge.

## Requirements & Acceptance Criteria

### 1. Domain Modeling (`TargetKind`)
- **AC1**: `HostEntity` carries `pub target_kind: TargetKind` where `TargetKind` is an enum with `Nixos`, `Agentless`, and `RemoteScript`.
- **AC2**: `HostRole::parse` supports `"router"`, `"embedded"`, `"cloud"`, `"desktop"`, `"notebook"`, `"server"`, and arbitrary strings via `HostRole::Unknown`.

### 2. Flake Output Discovery (`nodTargets`)
- **AC3**: `EvaluatorPort::discover_hosts` queries `nixosConfigurations` AND `nodTargets`.
- **AC4**: For `nodTargets.<name>`, if the target is a derivation, it is treated as `TargetKind::Agentless` with default metadata.
- **AC5**: For `nodTargets.<name>`, if the target is an attribute set `{ package, role, tags, targetHost, targetType, ... }`, metadata is extracted and mapped to `HostEntity`.
- **AC6**: If `nodTargets` does not exist in a flake, discovery silently proceeds with `nixosConfigurations` alone (100% backward compatibility).

### 3. Build & Activation Execution
- **AC7**: For `TargetKind::Nixos`, `build_toplevel` builds `<flake>#nixosConfigurations.<name>.config.system.build.toplevel`.
- **AC8**: For `TargetKind::Agentless` or `TargetKind::RemoteScript`, `build_toplevel` builds `<flake>#nodTargets.<name>.package` (or `<flake>#nodTargets.<name>`).
- **AC9**: For `TargetKind::Agentless`, the deployer executes the built activation artifact locally on the operator machine (invoking `bin/activate` or the package binary).
- **AC10**: Execution outcomes, timing, and errors are integrated into `DeploymentStateMachine`, `FleetSummary`, TUI views, and the JSON audit store without degradation.
