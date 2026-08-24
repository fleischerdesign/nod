# Spec: Consolidate SSH Connection-Field Duplication (W3)

**Module:** `src/domain/config.rs`, `src/infrastructure/config/toml_config.rs`.
**References:** ADR-004 (config hierarchy), ADR-007 (SshProfile flow).
**Depends on:** P1, P2 already in place.

## Problem

The 10-field SSH connection set (`user`, `port`, `identity_file`, `proxy_jump`,
`proxy_command`, `sudo`, `timeout_secs`, `connect_timeout_secs`, `extra_ssh_args`,
`allow_insecure`) is hand-declared in 6 structs. Adding a field requires editing all of
them in lockstep, and the serde-alias set exists on only one of them
(`SshProfileConfig`).

However, the six structs serve **different serialization shapes** and must NOT be
naively merged:

| Struct | Source | Shape | Keep as-is? |
|---|---|---|---|
| `SshProfileConfig` | Nix `config.nod` JSON | camelCase + aliases | Yes (distinct shape) |
| `SshOverrides` | TOML | snake_case, `#[serde(default)]` on `extra_ssh_args` | Yes (distinct TOML shape) |
| `TomlHost` | TOML flat + nested `ssh` | snake_case | Yes (flat+nested TOML design) |
| `Merged` | merge-only | embeds `SshOverrides` | Yes (already embeds) |
| `HostOverrides` | **programmatic** merge output | snake_case, Serialize+Deserialize | **Consolidate** |
| `FleetDefaults` | **programmatic** merge output | snake_case, Serialize+Deserialize | **Consolidate** |

`HostOverrides` and `FleetDefaults` are constructed programmatically (never TOML- nor
Nix-JSON-deserialized; the TOML store builds them in `toml_config.rs`). They are
near-identical: `HostOverrides` adds `target_host`/`role`/`tags`; everything else is the
same field set.

## Decision

Introduce one shared SSH-fields value object and embed it (flattened) into both
`HostOverrides` and `FleetDefaults`, so the 10-field set and its serde attrs are declared
once instead of twice, and the two structs can no longer drift.

### Design

```rust
/// The flat SSH connection override set, shared by HostOverrides and FleetDefaults.
/// Kept flat (no nesting) via #[serde(flatten)] so the serialized/constructed form of
/// the parent structs is unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshConnectionOverrides {
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
    pub proxy_jump: Option<String>,
    pub proxy_command: Option<String>,
    pub sudo: Option<bool>,
    pub timeout_secs: Option<u32>,
    pub connect_timeout_secs: Option<u32>,
    pub extra_ssh_args: Option<Vec<String>>,
    pub allow_insecure: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostOverrides {
    #[serde(flatten)]
    pub ssh: SshConnectionOverrides,
    pub target_host: Option<String>,
    pub description: Option<String>,
    pub role: Option<String>,
    pub tags: Option<Vec<String>>,
    pub build: Option<BuildConfig>,
    pub rollout: Option<RolloutConfig>,
    pub health_checks: Option<HealthCheckConfig>,
    pub hooks: Option<HooksConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetDefaults {
    #[serde(flatten)]
    pub ssh: SshConnectionOverrides,
    pub description: Option<String>,
    pub build: Option<BuildConfig>,
    pub rollout: Option<RolloutConfig>,
    pub health_checks: Option<HealthCheckConfig>,
    pub hooks: Option<HooksConfig>,
}
```

Because `#[serde(flatten)]` inlines the fields at the parent's top level, the flat
snake_case serialized/constructed shape of `HostOverrides`/`FleetDefaults` is **exactly
preserved** (no nesting, no serde-shape change). Field-accessor updates: callers that
read `host.user` / `host.port` / `host.identity_file` on a `HostOverrides` now read
`host.ssh.user` (etc.).

## Acceptance Criteria

- **AC1** — New `SshConnectionOverrides` type exists with the 10 fields, declared once.
- **AC2** — `HostOverrides` and `FleetDefaults` embed it via `#[serde(flatten)]`; the
  10-field list is removed from their direct bodies (declared once, not twice).
- **AC3** — Flattened serde shape preserved: any code that deserializes or constructs a
  `HostOverrides`/`FleetDefaults` with flat SSH keys still works. Update `toml_config.rs`
  where it constructs these types (the `Merged`→`HostOverrides`/`FleetDefaults` mapping
  at ~lines 422-455) to populate `ssh` with the `SshConnectionOverrides` from the merged
  `SshOverrides` fields instead of copying the 10 fields inline.
- **AC4** — All readers migrated to `host.ssh.<field>` where applicable. `grep` shows no
  remaining `HostOverrides`/`FleetDefaults` direct `.identity_file`/`.proxy_jump`/etc.
  field reads that bypass `.ssh.`.
- **AC5** — `SshOverrides`, `TomlHost`, `SshProfileConfig`, `SshProfile`, `Merged` are
  **untouched** (distinct shapes; explicitly out of scope).
- **AC6** — Tests: existing config tests (four-tier precedence, merged_for, CLI-wins,
  etc., from ADR-004) all still pass, proving flat-shape preservation. Add/keep a test
  that a `HostOverrides` with SSH fields round-trips through serialization and back with
  the same flat keys.

## Out of Scope
- `SshOverrides`/`TomlHost`/`SshProfileConfig`/`SshProfile`/`Merged` (distinct shapes).
- CLI clap args in `options.rs` (separate `#[arg(long)]` surface, out of scope).
- P4 (application/port cleanup), P5 (adapter hardening) — separate waves.

## Verification
- `cargo test` — all pass.
- `cargo clippy --all-targets -- -D warnings` — zero warnings.
- `cargo fmt --check`.
- `grep -rn "identity_file" src/domain/config.rs` shows it declared once (in
  `SshConnectionOverrides`) plus `SshProfileConfig`; not re-declared in
  `HostOverrides`/`FleetDefaults` bodies.
