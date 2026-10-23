# ADR-029: Post-activation health verdicts

- **Status:** Proposed
- **Date:** 2026-10-21
- **Deciders:** maintainers
- **Refines:** ADR-028 §6 (health and rollback). Does not change the trigger
  model, the state schema or the rollout waves.

## Context

ADR-028 §6 defines post-activation verification as one question: does
`systemctl --failed --no-legend --plain` return anything within
`healthChecks.timeoutSecs` (default 60)? That is the shipped implementation
(`src/infrastructure/health/probe.rs`).

Measured on 2026-10-21 in this fleet: `crowdsec.service` began failing at 12:19
and stayed in an automatic restart cycle for over four hours. `nod sync`
reported "fleet is at the remote revision" throughout, because:

1. **`--failed` is blind to restart loops.** A unit with `Restart=on-failure`
   that keeps dying is `ActiveState=activating`, `SubState=auto-restart`. It
   never appears in `--failed`, so the probe answers "healthy" by construction.
   Verbatim from the host: `systemctl is-active crowdsec` → `activating`.
2. **There is no baseline.** The verdict asks whether anything is failed now,
   not whether the deployment made anything worse. A host with a unit broken
   since last week fails every rollout, and a host that broke *this* deployment
   is indistinguishable from one that was already broken.
3. **Declared probes are parsed but never consulted.** `healthChecks.http_probes`
   and `custom_probes` exist in the TOML configuration, yet the verdict is
   computed from the failed-unit list alone.

The consequence is the failure mode ADR-028 was written to prevent: a tool that
reports green while the fleet is broken. A single boolean (`health_verified`)
cannot express what an operator needs to know.

## Decision

### 1. A verdict, not a boolean

Every host ends a deployment cycle in exactly one of these states:

| Verdict | Meaning | Rollout effect |
|---|---|---|
| `applied` | activation returned 0, verification still running | wait, never final |
| `healthy` | every enabled check passed, none was failing before | continue |
| `degraded` | a check that passed before the activation fails now | stop the wave; rollback policy applies |
| `degraded-pre-existing` | a check fails, but it failed before the activation too | report, do not attribute, continue |
| `pending-reboot` | the new configuration requires a reboot to take effect | not healthy, not broken; keep the host out of "converged" |
| `unverified` | no observation was possible (no SSH, device-API target, host offline, checks disabled) | never reported as healthy |

`unverified` is load-bearing. A host that cannot be observed is not a host that
is fine, and the fleet is converged only when every host is `healthy`,
`pending-reboot` or explicitly `unverified`.

### 2. What is checked

Not every unit. A host runs hundreds; checking all of them is slow, noisy, and
produces the one thing a health gate cannot survive: false alarms that teach
operators to ignore it.

Three sources, each independently switchable:

1. **Units this activation touched.** The deployer reads the unit lists from
   the activation output (`restarting`, `starting`, `reloading`, new units) and
   scores them after activation. This answers the sharpest question - is what I
   just changed actually running? Units the activation *stopped*, and changed
   units it deliberately did not restart, are not scored: a failed stop
   surfaces as a failed unit, and a unit stopped on purpose must never be
   required to run.
2. **Declared units** (`health_checks.units`). For services that may legitimately
   be quiet but must not be broken.
3. **Declared probes** (`health_checks.tcp_ports`, `http_probes`,
   `custom_probes`). For the failures no unit state can show: a web server
   answering 502 with a perfectly healthy unit, or a port nobody listens on.

### 3. How a unit is judged

Per unit, in this order:

- `LoadState=not-found` → unhealthy. A declared or touched unit that does not
  exist is a fault, not a silent pass: a typo in `required_units` used to look
  exactly like a healthy host.
- `ActiveState=failed` → unhealthy.
- `ActiveState=activating` with `SubState=auto-restart` → **unhealthy**. This is
  the restart loop `--failed` cannot see.
- `ActiveState=activating` otherwise → still starting; wait up to the host's
  `timeout_secs`, then unhealthy.
- `ActiveState=active` → healthy.
- `ActiveState=inactive` with `Result=success` for a `oneshot` unit → healthy.
- `ActiveState=inactive` otherwise → unhealthy **only** when the unit is
  declared or was touched; otherwise ignored.
- A declared TCP port must have a listening socket (`ss -ltnH`). A host
  without `ss` reports that as a problem instead of passing silently.
- `NRestarts` greater than the baseline plus `max_restarts` for a **declared**
  unit → unhealthy. Units the activation merely touched are not counted: the
  activation restarts them by design, and a counter that rises because the
  deployment did its job is not a regression.

Timer and socket units are never scored "inactive equals broken": a timer is
healthy when it is `active (waiting)`.

### 4. Baseline before activation

Immediately before activating a host, nod records the verdict inputs: the failed
set, the restart counters, and the probe results, with the same timeouts.

After activation:

- a check that failed before and fails after → `degraded-pre-existing`
- a check that passed before and fails after → `degraded`
- a check that did not exist before (new unit, new probe) → judged on its own,
  and a failure is `degraded`

The baseline is stored with the cycle's state so a later `nod audit` can answer
"was this host already broken when we deployed?" without re-probing.

If no baseline exists (first deployment, restored state, `--skip-baseline`), the
verdict is computed without attribution and any failure is `degraded` - the
conservative answer, stated explicitly rather than assumed.

### 5. Reboot-required deployments

When the activation reports `pending_reboot` (kernel, initrd, or a unit that
requires a reboot), the verdict is `pending-reboot` regardless of check results.
The host is neither healthy nor broken; it is not converged, and `nod sync`
names the hosts that owe a reboot instead of claiming the fleet is at the
revision.

### 6. Policy

The verdict feeds the existing policy, it does not replace it:

- `degraded` stops the current wave and triggers the rollback policy from
  ADR-028 §6 (on by default, `--no-auto-rollback` keeps it).
- `degraded-pre-existing` never triggers a rollback, because rolling back does
  not repair something the deployment did not break.
- `unverified` never triggers a rollback; it is reported and counted as not
  converged.
- Exit status follows ADR-027: a degraded host is a failed cycle.

### 7. Configuration

Every part is optional, and the defaults are the conservative ones. Nothing here
assumes this fleet's names, paths or services.

```toml
[hosts.web1.health_checks]
enable = true                 # false => verdict unverified, never healthy
timeout_secs = 60

[hosts.web1.health_checks.systemd]
check_failed_units = true
check_restart_loops = true
check_running = true         # judge the declared units below
check_changed_units = true   # judge the units this activation touched
required_units = ["sshd.service", "caddy.service"]
max_restarts = 0             # allowed automatic-restart increases
tcp_ports = [443, 8443]      # sockets that must be listening

[[hosts.web1.health_checks.http_probes]]
url = "https://web1.example/health"
expected_status = 200
timeout_secs = 5

[[hosts.web1.health_checks.custom_probes]]
name = "app-socket"
command = "test -S /run/app.sock"
timeout_secs = 5
```

Fleet-wide defaults live in `[health_checks]` and are overridden per host. A
host that declares nothing gets failed units and restart loops; declared units
and probes add to that. Baseline attribution is always on: a verdict without a
baseline is `degraded`, never `healthy`.

### 8. Reporting

`nod sync status` and `nod sync status --json` expose the verdict per host,
plus the reason when not healthy: unit name, state, and whether it was failing
before. They also print `deployed` and `verified` as separate revisions, so a
fleet at the remote revision with an unreachable host is not reported as
verified. A verdict without a reason cannot be acted on.

`nod status` stays the live reachability matrix: it probes now and does not read
stored state, so it cannot answer what the last deployment concluded.

### 9. A host cannot verify the activation that stops it

The process that activates a host is stopped by that activation when the host
is the control plane itself. Its verdict can never be written: the new daemon
starts and the old state keeps an `unverified` entry.

The daemon therefore probes the local host once at startup, before its first
cycle, and records the verdict in the state. There is no pre-activation
baseline after a restart; an existing problem is reported as the host's state
(`degraded`), not attributed to a deployment.

The same rule covers every other host between deployments. A deployer only
observes the hosts it activates, so the reconciler re-observes hosts whose
verdict is missing or older than `HEALTH_REFRESH_SECS` (half the freshness
window) without activating anything. An idle cycle may preserve the marker,
but only a complete, fresh observation can create it.

A fleet-wide `last_verified_rev` is set only when every discovered host is
reachable and its last recorded verdict is `healthy` — an aggregation of
stored observations, not a claim that a missing observation is healthy. Each
verdict carries `health_at`; only verdicts within `HEALTH_FRESHNESS_SECS`
(30 minutes) count, because a `healthy` from hours ago is not evidence for the
present.

A host that has been gone longer than the operator's `--gone-after` threshold
is outside that claim: the status still reports it as gone, and one sleeping
device must not make the marker permanently unattainable. An outage *inside*
the threshold still blocks, because that host may be about to come back.

## Consequences

- The boolean `health_verified` becomes `Option<HealthVerdict>`; every consumer
  either handles the verdict or loses information at compile time.
- The baseline costs one extra probe round per host per activation. For a fleet
  of five hosts that is milliseconds; for a fleet of hundreds, it is bounded by
  `concurrency` like every other SSH round.
- `degraded-pre-existing` is new behaviour: today such a host fails and rolls
  back. After this ADR it is reported and the rollout continues. That is a
  deliberate change of meaning, and it is the reason this is an ADR.
- Restart-loop detection reads `NRestarts` and `SubState`, which requires
  `systemctl show <unit>` per scored unit rather than one list command. The
  scored set is small by construction (§2).

## Rejected alternatives

- **Keep `systemctl --failed` and add a delay.** A restart loop with a 60 s
  cadence is invisible at any single sample; sampling twice helps only by luck.
- **Check every unit.** Hundreds of units per host, most irrelevant; noise
  guarantees the gate gets disabled.
- **Treat "no observation" as healthy.** It is the exact failure this ADR
  exists to remove.
- **Auto-rollback on `degraded-pre-existing`.** Rolls back a deployment for a
  fault it did not cause, and teaches operators to distrust the verdict.

## Implementation

Present in the code:

- `HealthSnapshot`, `HealthVerdict` and `verdict_against` in
  `src/domain/health.rs`: baseline attribution, restart-loop detection and the
  `degraded-pre-existing` distinction.
- The verdict is persisted per host (`HostMemory.health`), so `nod status` can
  name a degraded host without re-probing it.
- A startup self-check (`application/use_cases/self_check.rs`): the daemon
  probes the local host once after its own activation, records the verdict,
  and aggregates the stored verdicts for `last_verified_rev`.
- Declared units (`systemd.required_units`) and probes (`tcp_ports`,
  `http_probes`, `custom_probes`) are scored through one extra channel; each
  problem is a line in the snapshot. HTTP probes use `curl` and TCP ports use
  `ss`; a host without either reports that instead of passing silently.
- `NRestarts` counters for declared units are part of the baseline comparison:
  an increase beyond `systemd.max_restarts` (default 0) is a regression, even
  when the unit is `active` at the sample.
- The activation output is watched, not swallowed
  (`infrastructure/deployment/activation.rs`): every line is forwarded live to
  the operator, and the marker lines are kept for the parser. `restarting`,
  `starting`, `reloading` and new-unit lines become the touched set; colour
  codes are stripped, both separators (space and comma) are accepted, and a
  token that is not a unit is ignored. Stopped and deliberately unchanged units
  are excluded. `systemd.check_changed_units` (default true) switches the whole
  source off.
- A refresh pass (`application/use_cases/verify_fleet.rs`) re-observes hosts
  whose verdict is missing or stale, through the same deployer probe path
  (`DeployerPort::observe_health`); it never activates or copies a closure.
  `SyncState::mark_verified_when_all_healthy` is the single place that decides
  the marker, and it requires every selected host, not just the changed ones;
  hosts gone past `--gone-after` are reported as gone and do not block it.
- `last_applied_rev` (deployed to every host that answered) and
  `last_verified_rev` (every host settled and healthy) are separate facts.
  `nod status` prints `deployed` and `verified` on their own lines.
- A degraded activation maps to a failed cycle; `pending-reboot` and
  `unverified` are reported and never counted as converged.

## Open questions

- Whether `pending-reboot` should be its own exit status (ADR-027) or a flag on
  the report.
- Whether the baseline should be persisted beyond the cycle for `nod audit`, or
  recomputed. Persisted is preferred: an audit answer must not depend on the
  host still being reachable.
