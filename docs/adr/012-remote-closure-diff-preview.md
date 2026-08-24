# ADR-012: Polymorphic Remote & Local Closure Diff Preview

## Context
`nod diff` previously contained a branching implementation in `src/commands/diff.rs`:
- For local hosts (`host.is_local`), it compared `/run/current-system` against the freshly evaluated flake closure using `nvd diff` or `nix store diff-closures`.
- For remote hosts (`!host.is_local`), it executed a reachability check and stopped with a placeholder message: `"Remote host <name> is online. Ready for closure diff."`.

This violated clean code principles (incomplete abstraction, dead branch) and prevented operators from inspecting package and service changes on remote servers prior to triggering `nod switch`.

Both `LocalDeployer` and `SshCliDeployer` already implement `DeployerPort::current_closure(&self, host: &HostEntity, profile: &SshProfile) -> Result<Option<PathBuf>, NodError>`, which canonically queries the live `/run/current-system` store path locally or over SSH.

## Decision
1. **Polymorphic Closure Resolution**: Unify local and remote closure extraction under `DeployerPort::current_closure`, eliminating transport branching from `src/commands/diff.rs`.
2. **In-Sync Detection**: If the retrieved active closure matches the freshly built flake closure (`current == new_closure`), short-circuit diff execution and output a clean `✓ System is already in sync with target closure (no package changes).` message.
3. **Graceful Delta Tooling**: For differing closures, execute `nvd diff <current> <new_closure>` with automatic fallback to `nix store diff-closures <current> <new_closure>`.
4. **Initial Deployment Handling**: If no active closure is detectable (`None`), emit a warning noting that this is an initial deployment with no prior generation to compare against.

## Consequences
- **Positive**: Operators gain deep visibility into package and service updates on remote servers (`nod diff <host>`) prior to deployment.
- **Positive**: DRY and SOLID: Command logic is fully decoupled from local vs. SSH transport details.
- **Positive**: Instantaneous feedback when systems are already up-to-date without spawning heavy diff subprocesses.
