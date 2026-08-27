# ADR-024: Single-Batch Nix Flake Host Evaluation

## Context

Prior to this decision, host discovery in `NixCliEvaluator` executed a two-stage evaluation:
1. `nix eval --json <flake>#nixosConfigurations --apply builtins.attrNames` to list all host configuration names.
2. An asynchronous loop spawning $N$ individual `nix eval --json <flake>#nixosConfigurations.<host>.config --apply '...'` processes (bounded by a semaphore).

In large fleets (10–50+ hosts), invoking Nix $N$ separate times forced Nix to re-parse flake inputs, re-instantiate shared `nixpkgs` instances, and perform multiple separate evaluator warmups, leading to slow fleet discovery times.

## Decision

1. **Single-Batch Flake Evaluation**:
   - `NixCliEvaluator` evaluates all host configurations in a single `nix eval` invocation by applying a batch lambda:
     ```nix
     configs: builtins.mapAttrs (name: host: let x = host.config; in {
       targetHost = ...;
       role = ...;
       tags = ...;
       user = ...;
       port = ...;
       nod = if x ? nod then x.nod else null;
     }) configs
     ```
   - The resulting JSON map (`HashMap<String, FlakeMeta>`) is deserialized in one operation.

2. **Resilience & Degraded Discovery Fallback**:
   - If batch evaluation succeeds, all hosts are discovered in a fraction of a second.
   - If single-batch evaluation fails (e.g. when one host in the flake has a syntax/evaluation error and `--degraded` discovery is requested), the evaluator automatically falls back to per-host evaluation, isolating failing configurations while discovering all healthy hosts.

## Consequences

- **Discovery Latency**: Reduced from $O(N)$ separate process spawns and Nix evaluator initializations to a single $O(1)$ Nix evaluation call.
- **Resilience**: Full backward compatibility with degraded discovery mode is maintained via automatic fallback.
