# ADR-019: Store Optimization & Binary Cache Integration (`nod store optimize`, `nod cache push`)

## Context
As fleets grow and deployments accumulate, operators need:
1. **Store Optimization** (`nod store optimize`): Deduplicating identical files in the Nix store across local and remote fleet hosts via hardlinks (`nix-store --optimise`).
2. **Binary Cache Staging** (`nod cache push`): Pushing compiled system closures to remote binary caches (e.g. S3, SSH, HTTP caches like Attic, Cachix, or Harmonia) so target nodes can download pre-built binaries instead of building locally.
3. **Vendor Independence**: Relying on native Nix Store URI standards (`nix copy --to <URI>`) rather than proprietary vendor SDKs.

## Decision
1. **Domain Extensions (`src/domain/cache.rs`)**:
   - `StoreOptimizeReport`: outcome of hardlink deduplication per host.
   - `CachePushOptions` & `CachePushReport`: configuration and outcome of closure pushing to binary caches.
2. **SPI Port Extension (`src/domain/ports/store.rs`)**:
   - `StorePort::optimize_store(host: &HostEntity, profile: &SshProfile) -> Result<StoreOptimizeReport, NodError>`.
   - `StorePort::push_cache(host: &HostEntity, profile: &SshProfile, closure: &Path, cache_uri: &str) -> Result<CachePushReport, NodError>`.
3. **Infrastructure Implementations**:
   - `LocalDeployer` & `SshCliDeployer`: execute `nix-store --optimise` and `nix copy --to <cache_uri>` over local subprocess or SSH.
4. **Application Use Cases**:
   - `OptimizeStoreUseCase` (`src/application/use_cases/optimize_store.rs`).
   - `PushCacheUseCase` (`src/application/use_cases/push_cache.rs`).
5. **Presentation Layer**:
   - `nod store optimize [TARGET/GLOB] [--tag] [--role] [--all] [--json]`
   - `nod cache push [TARGET/GLOB] [--tag] [--role] [--all] [--cache URI] [--json]`

## Consequences
- **Positive**: Seamless remote store deduplication across the fleet.
- **Positive**: Native, universal binary cache synchronization using standard Nix URIs.
- **Positive**: Full adherence to Hexagonal Architecture, with 100% test coverage and zero compiler warnings.
