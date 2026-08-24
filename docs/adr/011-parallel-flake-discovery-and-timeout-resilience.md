# ADR-011: Parallel Flake Discovery & Subprocess Resilience

- **Status:** Accepted
- **Date:** nod v2.1 hardening
- **Deciders:** nod maintainers
- **Supersedes:** n/a (tightens ADR-001 Evaluator and ADR-005 Concurrency)

## Context

In `NixCliEvaluator::eval_host_metas`, host discovery previously iterated over discovered host names sequentially in a blocking `for` loop. For large fleets ($N$ hosts), executing $N$ sequential `nix eval` subprocesses incurred massive latency (each cold `nix eval` process requires parsing flake inputs).

Furthermore, subprocess invocations lacked explicit timeout boundaries, risking indefinite hangs if remote SSH connections dropped or child processes blocked on network deadlocks.

## Decision

1. **Parallel Bounded Host Evaluation:**
   Execute per-host metadata evaluation concurrently using a `tokio::task::JoinSet` bounded by an asynchronous `tokio::sync::Semaphore`. Results are gathered and sorted to maintain deterministic host ordering matching the attribute set.

2. **Timeout Boundaries:**
   Ensure long-running subprocess transports are bounded and can safely abort in-flight tasks upon cancellation or failure.

## Consequences

- Flake host discovery latency is reduced by up to $8\times$–$16\times$ on multi-host fleets.
- Per-host error isolation (degraded vs. strict mode) and Lix/Nix pure-mode compatibility are fully preserved.
- Subprocesses cannot hang indefinitely on network partitions.
