# ADR-018: Pluggable Secret Verification and Fleet Rekeying (`nod secret`)

## Context
Operators managing fleets of NixOS systems use various secrets management solutions, predominantly `sops-nix` (SOPS with Age/PGP) and `agenix` (Age encryption), as well as custom scripts or no secret tooling at all. Before performing multi-host deployments or rotating key material, operators need:
1. **Pre-flight verification** (`nod secret check`): Verify that all declared secrets for target hosts can be decrypted by the operator's current key material and that target hosts' public keys are included in the encryption recipient set.
2. **Fleet rekeying** (`nod secret rekey`): Re-encrypt secrets across the fleet when rotating age/SSH host keys.
3. **Pluggable & Vendor-Neutral**: Core domain logic must not be hardcoded to a single secret tool. It must automatically detect the configured provider (`sops`, `agenix`, `none`, `custom`) and gracefully handle hosts without secrets.

## Decision
1. **Domain Entities (`src/domain/secret.rs`)**:
   - `SecretProvider`: `Sops`, `Agenix`, `Custom(String)`, `None`.
   - `SecretStatus`: Individual secret file status (decryptable, recipient matched, error).
   - `SecretCheckReport`: Aggregated report per host (provider, count, valid, list of secret statuses).
   - `RekeyOptions` & `RekeyReport`: Options and result for secret re-encryption.
2. **SPI Port (`src/domain/ports/secret.rs`)**:
   - `SecretPort` trait with `check_secrets` and `rekey_secrets` methods.
3. **Infrastructure Adapter (`src/infrastructure/secret/pluggable_secret_store.rs`)**:
   - `PluggableSecretStore` implementing `SecretPort`:
     - Evaluates host configuration to auto-detect secret provider (`sops`, `agenix`, `custom`, `none`).
     - Delegates verification to appropriate backend (`sops`, `agenix`, or custom check command).
     - Returns graceful no-op report when no secrets are declared.
4. **Application Use Cases**:
   - `CheckSecretsUseCase` (`src/application/use_cases/check_secrets.rs`).
   - `RekeySecretsUseCase` (`src/application/use_cases/rekey_secrets.rs`).
5. **Presentation Layer**:
   - `nod secret check [TARGET/GLOB] [--tag] [--role] [--all] [--json]`
   - `nod secret rekey [TARGET/GLOB] [--tag] [--role] [--all] [--dry-run] [--no-backup] [--json]`

## Consequences
- **Positive**: Zero-configuration automated secret verification preventing broken deployments caused by undecryptable secrets or missing host recipients.
- **Positive**: Complete vendor independence through hexagonal ports and adapters.
- **Positive**: Strict quality gates with comprehensive unit tests for all provider variants.
