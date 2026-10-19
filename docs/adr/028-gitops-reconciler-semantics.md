# ADR-028: GitOps Reconciler Semantics (`nod sync`, `nod daemon`)

- **Status:** Accepted
- **Date:** 2026-10-19
- **Deciders:** maintainers
- **Supersedes:** ADR-022, section 2 (pull-based GitOps). `nod watch` is unaffected.

## Context

ADR-022 defines `nod sync` / `nod daemon` as a reconciler that "periodically
pulls from the upstream Git repository, builds the target closure into the Nix
store, and safely executes `nod switch` with automated health checks and
rollbacks".

The implementation shipped only the first half of the first step. Each cycle runs
`git fetch` and `git rev-parse HEAD`, then returns `SyncReport { applied: true }`
and prints "Successfully synchronized to commit …". It neither builds nor
activates, and never touches the deployer. A daemon built on that command would
report green while doing nothing — the worst possible failure mode for a
deployment tool.

The command is public and other Nix users run it. Redefining it is therefore a
deliberate, documented break, not a silent repair.

## Decision

### 1. What `sync` means

`nod sync` reconciles the running fleet towards one commit on one branch. It
reports `applied` only after activation was observed, and `changed: false` when
no host needed activation. There is no code path that reports success without
having attempted the deployment it claims.

### 2. Target selection is explicit

A daemon that silently deploys the whole fleet is the most damaging failure mode,
so there is no implicit fleet default:

- `--target <name|glob>`, `--tag`, `--role`, `--all`, or `local` choose the scope.
- Without an explicit scope and without `[sync].targets` in `.nod.toml`, the run
  fails with a configuration error. It never guesses.

### 3. Trigger model

- Each cycle starts with `git ls-remote <remote> <branch>` (one small request).
  Only a changed head triggers `git fetch`.
- Default interval 60 s; values below 15 s are rejected as a configuration error.
- Jitter of ±10 % by default, so many hosts sharing a repository do not poll in
  lockstep.
- Errors back off exponentially (30 s, 60 s, 120 s, … capped at 10 min) and reset
  on success. A broken commit is not rebuilt every cycle.

### 4. Change detection

1. A dirty worktree refuses synchronization: `sync` deploys commits, never local
   edits.
2. Integration is fast-forward only (`git merge --ff-only`). A non-fast-forward
   head means the history was rewritten; the reconciler refuses and reports,
   rather than resetting someone's checkout.
3. After the fast-forward, each host's toplevel is evaluated and compared with
   its running closure (`current_closure()`). Equal closures skip activation. On a
   fleet this is the main efficiency gain: a commit that touches one host leaves
   the others untouched.
4. A target whose revision is not a descendant of the last applied revision is a
   downgrade. It is refused unless `--allow-downgrade` is given.

### 5. Pipeline

```
preflight   lock, clean tree, remote, branch, free disk space
fetch       ls-remote gate → fetch → ff-only
evaluate    one batched evaluation (ADR-024)
build/copy  substituters first; local build only on a miss
deploy      waves (all | canary | batch) → activate → health → rollback on failure
report      per-host outcomes, exit status (ADR-027), audit entry, notify hooks
```

Per-host outcome states extend ADR-003 with `Skipped` (closure already current),
`Unreachable` (host did not answer) and `Refused` (preflight rejected the host).

### 6. Health and rollback

- Default health check: the activation returned successfully and
  `systemctl --failed` is empty on the host within `health_timeout_secs` (60 s).
- Optional per-host probes (HTTP, TCP, custom command) live in `.nod.toml` under
  `[hosts.<name>.health]`; a probe failure counts as an unhealthy host.
- Auto-rollback is **on** by default: an unhealthy host returns to its previous
  generation. `--no-auto-rollback` keeps the failed generation for inspection.
- A wave failure does not abort later waves unless `--fail-fast` is set; the exit
  status still reports the failure (ADR-027).

### 7. Self-deploy ordering

Deploying the host that runs the reconciler restarts services the reconciler may
depend on. Such hosts are deployed last:

- Default: the host executing `sync` is moved to the final wave when it is among
  the targets.
- `[sync].last_hosts = ["host", …]` extends that ordering for infrastructure the
  reconciler needs (gateway, ingress, DNS).

The daemon runs as its own systemd unit, outside any unit it deploys, so a
restart of a deployed service cannot kill the reconciler mid-run.

### 8. State

A versioned JSON file in `--state-dir` (default `/var/lib/nod`), because the
applied state is *derived* from closures and must never be trusted over them:

```json
{
  "version": 1,
  "repo": "github.com/example/nixfiles",
  "branch": "main",
  "last_attempt_rev": "…",
  "last_applied_rev": "…",
  "consecutive_failures": 0,
  "backoff_until": null,
  "in_progress": { "rev": "…", "started_at": "…", "hosts": ["…"] }
}
```

`in_progress` is written before the first activation and cleared only after a
complete report. A run that starts with a non-empty `in_progress` performs a
drift check instead of assuming the previous attempt succeeded.

A `flock` on the state directory admits exactly one reconciler per repository.

### 9. Configuration

All knobs live in `.nod.toml` under `[sync]`, following ADR-004 precedence:

```toml
[sync]
targets = ["local"]          # required unless --target/--all is given
remote = "origin"
branch = "main"
interval_secs = 60
jitter_percent = 10
state_dir = "/var/lib/nod"
auto_rollback = true
allow_downgrade = false
require_signature = false
health_timeout_secs = 60
strategy = "batch"           # all | canary | batch
batch_size = 1
fail_fast = false
concurrency = 4
last_hosts = []
notify = []                  # commands run after a report; receive JSON on stdin

[hosts.hom-srv-01.health]
http = ["http://10.10.100.10:8123/manifest.json"]
timeout_secs = 30
```

### 10. CLI

```
nod sync   [--once] [--dry-run] [--interval S] [--target T | --all]
           [--strategy S] [--batch-size N] [--concurrency N]
           [--auto-rollback | --no-auto-rollback] [--allow-downgrade]
           [--state-dir PATH] [--health-timeout S] [--json]
nod daemon [--interval S] [--state-dir PATH] [--json]
```

`--dry-run` reports drift per host — which hosts would change and to which
closure — and activates nothing. `--json` prints the same report machine-readable.

### 11. Exit status

Extends ADR-027:

| Code | Meaning |
|---|---|
| 0 | no change, or every targeted host completed |
| 1 | at least one host failed or was rolled back |
| 2 | configuration or usage error (including a missing target scope) |
| 3 | refused before deployment (dirty tree, non-fast-forward, downgrade, lock) |

### 12. Security

- **Who can push to the branch is who can become root on the fleet.** The
  control is branch protection with required review, documented as the operating
  requirement; the reconciler cannot substitute for it.
- The remote URL is pinned in configuration; an unknown remote is refused.
- No `reset --hard`, no force, no rewrite. Only fast-forward.
- `require_signature = true` verifies commits with `git verify-commit` before
  integrating. Off by default, because not every flake signs.
- Every cycle appends to the audit log: revision, host, outcome, duration.

## Consequences

- `nod sync` now performs the deployment it reports. A green run means the fleet
  runs the commit, or a documented `Skipped`/`Refused` reason exists.
- The default fleet scope is explicit, so a misconfigured daemon fails loudly
  instead of deploying everything.
- No host activation without drift; no rebuild after a failed commit within the
  backoff window.
- `nod sync --once` becomes usable from CI or a timer without a polling daemon.
- **Breaking:** ADR-022's GitOps section is superseded. This is a major release.
  No compatibility switch, no dual code path, no deprecation period.
- The state file is a convenience, never the source of truth; deleting it costs
  one drift check, not correctness.
- The reconciler depends on `current_closure()` being cheap and honest; hosts that
  cannot report their closure are `Unreachable`, never assumed current.

## Rejected alternatives

- **Report success after fetch** (today's behaviour): false green, rejected.
- **`reset --hard` to the remote head**: destroys local work and hides history
  rewrites instead of reporting them.
- **Deploy every cycle regardless of drift**: constant activation churn, restarts
  services that did not change.
- **Implicit "all hosts" default**: one typo turns a single-host deploy into a
  fleet rollout.
- **Applied state as source of truth**: drifts from reality the moment someone
  deploys manually; closures are the only honest signal.
- **Push-only webhook as the sole trigger**: requires an inbound endpoint on every
  fleet; kept as a future optional add-on, not the base mechanism.

## Implementation

Four PRs, each independently reviewable and green:

1. **Core:** `SyncState`, reconcile use case, git adapter (ls-remote, fetch,
   ff-only, ancestry), tests for no-change/change/refusal/backoff.
2. **CLI and config:** `[sync]` section, new flags, `--dry-run` drift report,
   `--json`, exit codes, removal of the fetch-only path.
3. **Deployment:** integrate `DeployFleetUseCase`, `Skipped`/`Unreachable`/`Refused`
   states, self-deploy ordering, health probes, auto-rollback wiring.
4. **Daemon and docs:** systemd unit template, crash recovery from `in_progress`,
   audit and notify hooks, operator documentation including the branch-protection
   requirement.
