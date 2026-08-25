# ADR-017: Fleet Orchestration Reboot (`nod reboot`)

## Context
After system deployments and kernel/driver upgrades (as surfaced by `nod info`'s `reboot pending` indicator), operators need an orchestrated, safe reboot workflow across one or more fleet hosts:
1. Orchestration: Ability to reboot hosts according to rollout strategies (`batch`, `canary`, `all`) with concurrency bounding to avoid simultaneous fleet outages.
2. Verification: Automatic polling after reboot command issuance until the node drops connection and comes back online with active SSH and healthy systemd status.
3. Resilience: Graceful handling of SSH socket termination on remote reboot (`EXIT_SSH_DISCONNECT` / code 255).
4. Auditability: Persistent audit recording of reboot events in `~/.local/share/nod/history.json`.

## Decision
1. **Domain Extensions**:
   - `DeployerPort::reboot(host: &HostEntity, profile: &SshProfile) -> Result<(), NodError>`
   - `RebootOptions`, `RebootOutcome`, `RebootSummary`.
2. **Infrastructure Adapters**:
   - `LocalDeployer::reboot`: spawns `sudo reboot` or `systemctl reboot`.
   - `SshCliDeployer::reboot`: runs `sudo reboot` over SSH, explicitly treating network socket disconnect as successful launch.
3. **Application Use Case**:
   - `RebootFleetUseCase`: drives progressive multi-host reboot pipelines with health-poll loop and timeout monitoring.
4. **Presentation Layer**:
   - `src/commands/reboot.rs` (`nod reboot [TARGET/GLOB] [--tag] [--role] [--all] [--strategy STRATEGY] [--batch-size N] [--concurrency N] [--no-wait] [--timeout SECS]`).

## Consequences
- **Positive**: Zero-downtime rolling reboots across fleet clusters.
- **Positive**: Automated confirmation that nodes successfully recover and enter a healthy operational state.
- **Positive**: Cohesive integration with the existing `DeployerPort`, `HealthCheckerPort`, and `AuditStorePort`.
