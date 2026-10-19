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
- Without an explicit scope, the run fails with a configuration error. It
  never guesses. A scope is `--target`/`--tag`/`--role`/`--all`.

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

- Activation success is the baseline: a host whose switch returned non-zero is
  a failed host, and the rollback policy applies.
- Where the pipeline has a health checker bound, the host must also pass it. The
  local systemd checker verifies `systemctl --failed`. Remote verification is
  the SSH deployer's responsibility and is **not implemented yet** - a remote
  host counts as healthy once its activation succeeded. The gap is recorded here
  so "health checks" is never read as more than it is.
- Probe definitions stay in the fleet's `options.nod.healthChecks`; the
  reconciler adds no second vocabulary.
- Auto-rollback is **on** by default: an unhealthy host returns to its previous
  generation. `--no-auto-rollback` keeps the failed generation for inspection.
- A wave failure does not abort later waves unless `--fail-fast` is set; the exit
  status still reports the failure (ADR-027).

### 7. Self-deploy ordering

Deploying the host that runs the reconciler restarts services the reconciler may
depend on. Such hosts are deployed last:

- Default: the host executing `sync` is moved to the final wave when it is among
  the targets.
- `--last-host <name>` extends that ordering for infrastructure the
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

A PID lock file in the state directory admits exactly one reconciler per
repository. A lock whose owner is gone is reclaimed; without a process probe
(e.g. a non-Linux host) it is reclaimed by age instead.

### 9. Configuration

Every knob is a CLI flag. The daemon's persistent configuration is its systemd
unit — in a NixOS fleet the module options that render that unit — and not a
second `.nod.toml [sync]` tier:

```
remote, branch, interval, jitter, state-dir,
strategy, batch-size, concurrency, fail-fast, no-auto-rollback,
allow-downgrade, require-signature, last-host, notify, json, dry-run
```

Rationale: a daemon started by systemd already has one declarative source for
its arguments. Duplicating them in `.nod.toml` would create two places to
change one interval and force a precedence rule nobody can remember on a host
nobody edited. Interactive `nod sync` takes the same flags from the shell.

A unit that wraps these flags is a template, not part of the binary: the
NixOS module in the consuming fleet owns it.

### 10. CLI

```
nod sync   [--once] [--dry-run] [--target T | --all] [--tag TAG] [--role ROLE]
           [--interval S] [--jitter P] [--state-dir PATH]
           [--strategy S] [--batch-size N] [--concurrency N] [--fail-fast]
           [--no-auto-rollback] [--allow-downgrade] [--require-signature]
           [--last-host HOST] [--notify COMMAND] [--json]
nod daemon [same flags, without --once]
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

Delivered in one change, because the pieces only mean something together:

1. **Domain:** `GitPort`, `SyncStatePort`, `SyncState`/`SyncReport`, extended
   `SyncOptions`, `NodError::Refused`.
2. **Adapters:** `CliGit` (ls-remote, fetch, fast-forward only, ancestry,
   signature) and `JsonSyncStateStore` with an atomic write and a PID lock.
3. **Application:** `SyncReconcilerUseCase` — preflight, revision gate,
   signature and downgrade checks, drift filtering, deployment through the
   existing `DeployFleetUseCase`, self-deploy ordering, report and backoff.
4. **Presentation:** `nod sync`, `nod daemon`, exit statuses (0/1/2/3), audit
   entries, notification hooks.

Deliberately left out, and named here so the omission is a decision:

- A `.nod.toml [sync]` section (see §9).
- A push/webhook trigger; the polling gate is the base mechanism.
- Health-probe definitions of its own; the fleet's `options.nod.healthChecks`
  stays the single vocabulary.
- A systemd unit template in this repository; the consuming fleet's NixOS
  module renders it from the flags.
