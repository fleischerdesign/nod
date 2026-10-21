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

Two cadences, one truth. The **sweep** is the authoritative cycle: it probes,
evaluates and deploys. The **gate** is a cheap remote-head check that may start
a sweep earlier than its cadence.

- The gate runs `git ls-remote <remote> <branch>` every `--gate-interval`
  (default 15 s) and compares the head with its previous observation. A change
  starts a sweep immediately, so a push is noticed in seconds rather than an
  interval.
- The gate never *replaces* a sweep. Host drift happens without a push - a
  manual `nixos-rebuild`, a reboot into an older generation, a dead unit - so
  the sweep keeps its own cadence (`--interval`, default 60 s) whether or not
  the remote moved.
- `--gate-interval 0` disables the gate. That is the right setting for a
  repository on a local path, where `ls-remote` has no meaning.
- Values below 5 s are rejected; the sweep still requires at least 15 s.
- Jitter of ±10 % applies to both cadences, so many hosts sharing a repository
  neither poll nor check in lockstep.
- Errors back off exponentially (30 s, 60 s, 120 s, … capped at 10 min).
- A backoff belongs to the revision that failed. A newer remote head clears it:
  waiting out a pause for code that cannot be its cause is pure delay, and that
  pause is precisely what delays a fix.

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
evaluate    one batched evaluation per host, concurrent under --concurrency
build/copy  substituters first; local build only on a miss
deploy      waves (all | canary | batch) → activate → health → rollback on failure
report      per-host outcomes, exit status (ADR-027), audit entry, notify hooks
```

Per-host results are reported in the sync vocabulary - `Deployed`, `Unchanged`,
`RolledBack`, `Failed`, `Unreachable`, `Skipped` - mapped from the pipeline's
terminal states. They deliberately do not extend the ADR-003 state machine: a
skipped or unreachable host never enters a deployment lifecycle at all.

Reachability is checked before evaluation, in parallel with the rest of the
sweep. What a probe learned is remembered per host, in one of three conditions:

- `reachable` - the host answered.
- `offline` - the host did not answer. The next probe is deferred by an
  exponential delay (60 s, 120 s, … capped at 30 min by default), so an offline
  laptop costs *no* probe instead of one per cycle. An outage longer than
  `--gone-after` (24 h by default) is labelled `gone` in `nod sync status` - a
  label, never a removal: the inventory is the truth, and a wrong address must
  not look like success.
- `misconfigured` - the host answers but refuses our credentials. This is a
  typed condition (`NodError::Auth`), never conflated with an outage: a wrong
  deploy key needs an operator, not time. Conflating the two is how a broken
  rollout hides behind a plausible label.

Only hosts that answered *and* drifted enter the rollout. A designated canary
therefore cannot be an unreachable host and cannot hold back the fleet, and no
election mechanism is needed: under `--strategy canary` the first host in
deployment order is logged as the canary, so the journal states the order
instead of leaving it to inference.

### 6. Health and rollback

- Activation success is the baseline: a host whose switch returned non-zero is
  a failed host, and the rollback policy applies.
- After activation every host is probed with `systemctl --failed --no-legend
  --plain` - locally for the local host, over the same SSH transport for remote
  hosts - bounded by the host's `options.nod.healthChecks.timeoutSecs` (default
  60 s). Any failed unit fails the host and triggers the rollback policy.
- The probe honours the host's configuration: `healthChecks.enable = false`
  skips verification, `healthChecks.systemd.checkFailedUnits = false` skips the
  unit check.
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
  "version": 2,
  "repo": "github.com/example/nixfiles",
  "branch": "main",
  "last_attempt_rev": "…",
  "last_applied_rev": "…",
  "consecutive_failures": 0,
  "backoff_until": null,
  "backoff_rev": null,
  "in_progress": { "rev": "…", "started_at": "…", "hosts": ["…"] },
  "hosts": {
    "hom-srv-01": {
      "condition": { "kind": "offline", "since": 0, "attempts": 3, "next_probe_at": 0 },
      "last_seen": 0
    }
  }
}
```

`in_progress` is written before the first activation and cleared only after a
complete report. A run that starts with a non-empty `in_progress` performs a
drift check instead of assuming the previous attempt succeeded.

`backoff_rev` is the revision the current pause belongs to; a different remote
head clears the pause. `hosts` is the probe memory described in §5. Both are
memory, not truth: a document written by an older schema version is discarded
whole, which costs one sweep and nothing else.

A PID lock file in the state directory admits exactly one reconciler per
repository. A lock whose owner is gone is reclaimed; without a process probe
(e.g. a non-Linux host) it is reclaimed by age instead.

### 9. Configuration

Every knob is a CLI flag. The daemon's persistent configuration is its systemd
unit — in a NixOS fleet the module options that render that unit — and not a
second `.nod.toml [sync]` tier:

```
remote, branch, interval, gate-interval, jitter, state-dir,
offline-backoff, offline-backoff-max, gone-after,
strategy, batch-size, concurrency, fail-fast, no-auto-rollback,
allow-downgrade, require-signature, last-host, notify, json, dry-run
```

The cadence knobs are separable on purpose. A fleet of servers keeps the gate
short and the sweep short; a fleet with a laptop that sleeps for days leaves the
gate short and stretches `--offline-backoff` and `--gone-after`. None of that
requires a fork, and none of it is a constant in the loop.

Rationale: a daemon started by systemd already has one declarative source for
its arguments. Duplicating them in `.nod.toml` would create two places to
change one interval and force a precedence rule nobody can remember on a host
nobody edited. Interactive `nod sync` takes the same flags from the shell.

A unit that wraps these flags is a template, not part of the binary: the
NixOS module in the consuming fleet owns it.

### 10. CLI

```
nod sync   [--once] [--dry-run] [--target T | --all] [--tag TAG] [--role ROLE]
           [--interval S] [--gate-interval S] [--jitter P] [--state-dir PATH]
           [--offline-backoff S] [--offline-backoff-max S] [--gone-after S]
           [--strategy S] [--batch-size N] [--concurrency N] [--fail-fast]
           [--no-auto-rollback] [--allow-downgrade] [--require-signature]
           [--last-host HOST] [--notify COMMAND] [--json]
nod sync status [TARGET | --all] [--tag TAG] [--role ROLE] [--drift] [--json]
nod daemon [same flags as sync, without --once]
```

Read-only views are subcommands of the verb they describe (`nod sync status`),
not a separate hyphenated top-level command: the command surface has one word
per command, and a view of `sync` is not a verb of its own.

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
