# ADR-015: Store and Generations Management (Generations, GC, Copy)

## Context
Operators need robust capabilities to inspect, clean up, and stage NixOS system generations across the fleet:
1. `nod generations`: List historical and active profile generations on local and remote hosts without requiring root locks.
2. `nod gc`: Execute garbage collection on local and remote hosts, with options for generation retention (`--keep N`), age expiration (`--older-than X`), and dry-run previewing.
3. `nod copy`: Pre-stage store closures on target hosts without executing configuration switches or touching the active bootloader.

## Decision
1. **Domain Port `StorePort`**:
   - `list_generations(host: &HostEntity, profile: &SshProfile) -> Result<Vec<SystemGeneration>, NodError>`
   - `collect_garbage(host: &HostEntity, profile: &SshProfile, options: &GcOptions) -> Result<GcReport, NodError>`
   - `copy_closure(host: &HostEntity, profile: &SshProfile, closure: &Path, to_remote: bool) -> Result<CopyReport, NodError>`
2. **Infrastructure Adapters**:
   - `LocalStoreDeployer` and `SshCliStoreDeployer` implement `StorePort`.
   - `list_generations` parses `/nix/var/nix/profiles/system*` symlink targets and timestamps via `stat`, guaranteeing non-blocking read access without database locks.
   - `collect_garbage` wraps `nix-collect-garbage` with configurable retention policies and sudo support.
   - `copy_closure` executes `nix copy --to` or `nix copy --from` with SSH transport settings.
3. **Application Use Cases**:
   - `ListGenerationsUseCase`: Collects generations across resolved targets (parallelized across hosts).
   - `CollectGarbageUseCase`: Executes garbage collection bounded by concurrency budget.
   - `CopyClosureUseCase`: Evaluates or builds the closure and stages it onto targets.
4. **Presentation Layer**:
   - `nod generations [TARGET/GLOB] [--tag] [--role] [--all] [--json]`
   - `nod gc [TARGET/GLOB] [--tag] [--role] [--all] [--keep N] [--older-than X] [--dry-run] [--concurrency N]`
   - `nod copy [TARGET/GLOB] [--tag] [--role] [--all] [--to URL] [--from URL]`
5. **Composition Root**:
   - Bind `StorePort` in `src/commands/wiring.rs::production`.

## Consequences
- **Positive**: Full visibility into generation history across all fleet nodes.
- **Positive**: Safe disk space reclamation with fine-grained retention controls.
- **Positive**: Ability to pre-warm remote caches and stage deployments prior to switch windows.
- **Positive**: Strict adherence to Hexagonal Architecture, with pure mockable domain ports and typed `NodError` errors.
