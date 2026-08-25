# nod — Universal Nix Orchestration & Deployment Engine

[![Quality Gate](https://img.shields.io/badge/Quality%20Gate-100%25%20Passing-brightgreen.svg)]()
[![Architecture](https://img.shields.io/badge/Architecture-Hexagonal%20%2F%20Clean-blue.svg)]()
[![Rust](https://img.shields.io/badge/Rust-2021%20Edition-orange.svg)]()

`nod` is a production-grade, universal NixOS orchestration and deployment engine built with Rust, Tokio, Ratatui, and Clap. It discovers NixOS flake configurations, evaluates closures, stages artifacts, coordinates rolling fleet deployments, manages profile generations and garbage collection, evaluates expressions, and verifies system health with automatic rollback capabilities.

---

## 🚀 Key Features

* **Universal Target Selector**: Filter and target hosts seamlessly using `[TARGET/GLOB]`, `--tag <TAG>`, `--role <ROLE>`, or `--all`.
* **Deployment Lifecycle**: Safe progressive deployments (`switch`, `test`, `boot`, `build`, `plan`, `rollback`) with canary/batch wave strategies and auto-rollback boundaries.
* **Store & Profile Management**: Inspect generation history (`nod generations`), pre-stage closures (`nod copy`), and perform orchestrated garbage collection (`nod gc`).
* **Flake & Lockfile Operations**: Tree-view flake inputs (`nod inputs`), inspect repository metadata (`nod metadata`), and perform locked updates with git revision diffs (`nod update`).
* **Developer & Inspection Tools**: Evaluate expressions in host configuration context (`nod eval`), spawn pre-loaded interactive sessions (`nod repl`), and view full telemetry cards (`nod info`).
* **Fleet Orchestration & Operations**: Rolling host reboots (`nod reboot`), parallel multi-host command execution (`nod exec`), and interactive SSH sessions (`nod ssh`).
* **Interactive TUI Dashboard**: Live matrix view, host details, and real-time audit trail powered by Ratatui (`nod dashboard`).

---

## 📖 CLI Command Reference

### Deployment & Lifecycle
```bash
# Deploy and activate on a single machine or across a fleet
nod switch [TARGET/GLOB] [--tag <TAG>] [--role <ROLE>] [--all] [--strategy batch|canary]

# Temporarily test a configuration without modifying the bootloader
nod test [TARGET/GLOB] [--all]

# Set a configuration as default for next boot without live switching
nod boot [TARGET/GLOB] [--all]

# Build system toplevel closure (optionally using a dedicated builder node)
nod build [TARGET] [--builder <BUILDER_HOST>] [--out-link <PATH>]

# Dry-run deployment plan with closure and package diff preview
nod plan [TARGET/GLOB] [--all]

# Revert a host to its previous known-good generation
nod rollback [TARGET]
```

### Flake & Lockfile Management
```bash
# List flake input hierarchies, URLs, and follow relationships
nod inputs [--flake <PATH>] [--json]

# Inspect flake repository status, git revision, and lockfile version
nod metadata [--flake <PATH>] [--json]

# Update specific flake inputs or all inputs with before/after commit deltas
nod update [INPUTS...] [--flake <PATH>] [--json]

# Run strict repository quality gates (nixfmt + deadnix + statix)
nod check [--flake <PATH>]
```

### Generations & Garbage Collection
```bash
# Inspect installed system profile generations across hosts
nod generations [TARGET/GLOB] [--all] [--json]

# Collect garbage and delete obsolete generations with retention rules
nod gc [TARGET/GLOB] [--all] [--keep 5] [--older-than 14d] [--dry-run] [--json]

# Pre-stage system closures on remote nodes without activating
nod copy [TARGET/GLOB] [--all] [--to <URI>] [--from <URI>]
```

### Secrets & Security
```bash
# Pre-flight secret decryptability and recipient verification (sops / agenix / custom)
nod secret check [TARGET/GLOB] [--all] [--json]

# Fleet-wide secret re-encryption and recipient rotation
nod secret rekey [TARGET/GLOB] [--all] [--dry-run] [--no-backup] [--json]
```

### Developer, Diagnostics & Fleet Tools
```bash
# Evaluate arbitrary Nix expressions in host context
nod eval 'config.services.nginx.enable' [TARGET] [--all] [--raw] [--json]

# Open interactive nix repl pre-seeded with { flake, host, config, options, pkgs }
nod repl [TARGET]

# View comprehensive diagnostic card (kernel, uptime, generations, systemd health)
nod info [TARGET/GLOB] [--all] [--json]

# Rolling coordinated reboot with automatic recovery verification
nod reboot [TARGET/GLOB] [--all] [--strategy batch] [--batch-size 1] [--timeout 180]

# Parallel remote command execution across the fleet
nod exec [TARGET/GLOB] [--all] [--sudo] -- uname -a

# Open interactive SSH shell or run remote command
nod ssh [TARGET] [--sudo]

# Launch interactive Ratatui TUI dashboard
nod dashboard

# View persistent deployment and rollback audit history
nod audit [TARGET] [--limit 20] [--json]
```

---

## 🏛 Architecture

`nod` strictly adheres to **Hexagonal Architecture (Clean / 4 Layers)**:

1. **Domain Layer (`src/domain/`)**: Pure business logic, value objects, entities (`HostEntity`, `HostRole`, `SshProfile`, `SystemGeneration`, `EvalResult`, `HostInfo`), and SPI Port traits (`EvaluatorPort`, `DeployerPort`, `StorePort`, `FlakePort`, `AuditStorePort`, `HealthCheckerPort`). Zero I/O or external tool dependencies.
2. **Application Layer (`src/application/`)**: Use cases orchestrating operations (`SwitchUseCase`, `ListGenerationsUseCase`, `CollectGarbageUseCase`, `EvalFleetUseCase`, `InspectInfoUseCase`, `RebootFleetUseCase`), target selection, and `AppContext` (DI container).
3. **Infrastructure Layer (`src/infrastructure/`)**: Concrete adapters implementing SPI ports (`NixCliEvaluator`, `SshCliDeployer`, `LocalDeployer`, `NixCliFlakeStore`, `JsonAuditStore`, `SystemdChecker`, `TomlConfigStore`).
4. **Presentation Layer (`src/commands/` & `src/ui/`)**: Presentation glue wiring Clap CLI commands and Ratatui TUI to application use cases via `wiring::production(...)`.

---

## 🧪 Quality Gate & Testing

All contributions must pass the strict Quality Gate with 0 warnings:

```bash
# Run entire test suite (100% pass required)
cargo test

# Run strict linter (zero-warning policy)
cargo clippy --all-targets -- -D warnings

# Verify formatting
cargo fmt --check

# Build release binary or Nix package
cargo build --release
# or
nix build
```

---

## 📜 Documentation

* **Architecture Decision Records**: [`docs/adr/`](docs/adr/)
* **Specifications**: [`docs/spec/`](docs/spec/)
* **Architecture Roadmap**: [`docs/architecture/roadmap.md`](docs/architecture/roadmap.md)
* **Architecture Overview**: [`docs/architecture/overview.md`](docs/architecture/overview.md)
