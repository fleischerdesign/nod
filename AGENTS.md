# AGENTS.md — nod (Universal Nix Orchestration & Deployment Engine)

## Tech Stack & Tooling
- **Language**: Rust (Edition 2021, Library + Binary Crate), Tokio Async Runtime
- **CLI & TUI**: Clap 4.5 (derive), Ratatui 0.26 + Crossterm 0.27
- **Nix Module**: `modules/nixos/default.nix` exported via `flake.nix`

## Essential Commands (Quality Gate)
- **Test**: `cargo test` (Must pass 100%, 0 failures)
- **Linter**: `cargo clippy --all-targets -- -D warnings` (Strict zero-warning policy)
- **Format**: `cargo fmt --check`
- **Build**: `nix build` or `cargo build`

## Architecture Invariants (Non-Negotiable)
1. **Hexagonal Architecture (Clean / 4 Layers)**:
   - `domain/`: Pure business logic, entities, value objects, SPI port traits. ZERO I/O or subprocess dependencies.
   - `application/`: Use cases, pipeline state machine, target selection, and `AppContext` (DI container).
   - `infrastructure/`: Adapters implementing ports (`nix/`, `deployment/`, `config/`, `health/`, `storage/`).
   - `commands/` & `ui/`: Presentation layer; wires `AppContext` to CLI commands and TUI.
2. **Error Handling**: Use strongly typed `thiserror` enums (`NodError`) in domain/app layers. No opaque `anyhow!` in core.
3. **Configuration Cascade**: CLI Flags > `.nod.toml` > Flake (`config.nod.*`) > System Defaults.
4. **Universal Target Selector**: All host commands use `TargetSelection::select` (`[TARGET/GLOB] [--tag] [--role] [--all]`).

## Commit & PR Conventions
- Conventional Commits: `feat(...)`, `fix(...)`, `refactor(...)`, `docs(...)`, `test(...)`.
- Co-locate specifications in `docs/spec/<feature>.spec.md` and ADRs in `docs/adr/`.