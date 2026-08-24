# ADR-014: Flake and Lockfile Management (Inputs, Metadata, Update)

## Context
Operators need direct visibility into and control over the flake input ecosystem powering their NixOS fleet:
1. `nod inputs`: Inspect the hierarchy of configured flake inputs, original sources, locked revisions, and override/follows mappings.
2. `nod metadata`: Inspect repository/flake metadata, including revision counts, last modified date, and lockfile version.
3. `nod update [INPUTS...]`: Selectively or globally update flake inputs, calculating and rendering an exact before/after revision and date delta summary.

In accordance with ADR-001 (Hexagonal Architecture), flake inspection and mutation must not couple the Presentation layer directly to shell calls. Instead, operations are mediated through a domain port (`FlakePort`), pure application use cases (`ListInputsUseCase`, `InspectMetadataUseCase`, `UpdateFlakeUseCase`), and an infrastructure adapter (`NixCliFlakeStore`).

## Decision
1. **Domain Port `FlakePort`**:
   - `load_metadata(flake_path: &Path) -> Result<FlakeMetadata, NodError>`
   - `load_inputs(flake_path: &Path) -> Result<Vec<FlakeInputNode>, NodError>`
   - `update_inputs(flake_path: &Path, inputs: &[String]) -> Result<FlakeUpdateReport, NodError>`
2. **Application Use Cases**:
   - `ListInputsUseCase`: Retrieves and flattens or structures the flake inputs tree for display.
   - `InspectMetadataUseCase`: Retrieves root metadata (revision, lock version, input count, last modified).
   - `UpdateFlakeUseCase`: Snapshots `flake.lock`, executes `nix flake update`, and computes the exact difference between old and new locked revisions.
3. **Presentation Layer**:
   - `nod inputs [--flake PATH] [--json]`: Renders a tree/table of all inputs.
   - `nod metadata [--flake PATH] [--json]`: Renders high-level flake summary statistics.
   - `nod update [INPUTS...] [--flake PATH] [--commit]`: Updates inputs and prints a colored delta table.
4. **Composition Root**:
   - Bind `FlakePort` in `src/commands/wiring.rs::production`.

## Consequences
- **Positive**: Complete observability and control of flake inputs from within `nod`.
- **Positive**: Clean hexagonal boundaries: CLI commands only interact with use cases and typed entities.
- **Positive**: Machine-readable JSON output supported across all inspection commands.
- **Positive**: Safe updates with explicit revision delta reporting.
