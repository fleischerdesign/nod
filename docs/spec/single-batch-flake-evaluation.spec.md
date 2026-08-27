# Specification: Single-Batch Nix Flake Host Evaluation

## Purpose
Accelerate host matrix discovery across fleets by evaluating all NixOS configurations in a single Nix evaluator call.

## Requirements & Acceptance Criteria

### 1. Batch Host Evaluation
- **AC1**: `NixCliEvaluator` MUST attempt single-batch evaluation of `<flake>#nixosConfigurations` using `builtins.mapAttrs`.
- **AC2**: Deserialization MUST parse all hosts, roles, tags, connection descriptors, and `config.nod` settings into `FlakeMeta` mappings.
- **AC3**: Deterministic host order matching the flake's attribute set keys MUST be preserved.

### 2. Degraded Mode & Fallback
- **AC4**: If single-batch evaluation fails and degraded discovery is enabled, the evaluator MUST fall back to per-host evaluation.
- **AC5**: Per-host errors in degraded mode are reported as warnings, and healthy hosts are returned.
- **AC6**: In strict mode, an evaluation error MUST immediately propagate as a typed `NodError`.
