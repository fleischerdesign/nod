# ADR-021: Day-0 Provisioning, Scaffolding & ISO Generation (`nod bootstrap`, `nod init`, `nod iso`)

## Context
Before an operator can perform lifecycle management (`nod switch`, `reboot`, `gc`) on a NixOS fleet, they encounter Day-0 challenges:
1. **Scaffolding Fresh Repositories** (`nod init`): Creating a standard, best-practice Nix flake repository pre-configured with `nod` modules, sane tier-3 defaults, and directory structures (`hosts/`, `modules/`).
2. **Bare-Metal & Remote Provisioning** (`nod bootstrap`): Bootstrapping a blank physical machine or cloud VM from a live ISO via `nixos-anywhere` and `disko`.
3. **Custom Installation Media** (`nod iso`): Generating bootable installer ISOs or VM images pre-seeded with operator SSH keys and host configurations via NixOS generation outputs.

## Decision
1. **Domain Entities (`src/domain/provision.rs`)**:
   - `BootstrapOptions` & `BootstrapReport`.
   - `InitTemplate` (`Minimal`, `Fleet`, `Server`) & `InitOptions`.
   - `IsoOptions` & `IsoReport`.
2. **SPI Port (`src/domain/ports/provisioner.rs`)**:
   - `ProvisionerPort` with `bootstrap` and `build_iso` methods.
3. **Infrastructure Adapter (`src/infrastructure/provision/nixos_anywhere_provisioner.rs`)**:
   - `NixosAnywhereProvisioner` implementing `ProvisionerPort`:
     - Spawns `nixos-anywhere` with target parameters (`--flake <flake>#<host>`, target IP, disko mode).
     - Gracefully reports missing binary dependencies with actionable installation advice.
     - Builds bootable ISO artifacts using Nix flake outputs.
4. **Application Use Cases**:
   - `BootstrapHostUseCase` (`src/application/use_cases/bootstrap_host.rs`).
   - `ScaffoldFlakeUseCase` (`src/application/use_cases/scaffold_flake.rs`).
   - `GenerateIsoUseCase` (`src/application/use_cases/generate_iso.rs`).
5. **Presentation Layer**:
   - `nod bootstrap <TARGET> --ip <IP> [--user root] [--port 22] [--disko] [--no-kexec] [--debug] [--json]`
   - `nod init [DIR] [--template minimal|fleet|server] [--name <NAME>]`
   - `nod iso [TARGET] [--format iso|qcow2] [--json]`

## Consequences
- **Positive**: Complete end-to-end lifecycle from empty directory (`nod init`) to installed server (`nod bootstrap`) to fleet day-2 operations (`nod switch`).
- **Positive**: Zero-touch bootstrapping using industry-standard `nixos-anywhere` and `disko`.
- **Positive**: Strict port isolation maintaining domain purity and 100% test pass.
