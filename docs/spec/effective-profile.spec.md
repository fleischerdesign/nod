# Spec: Effective SshProfile Flows Through the Deploy Port (ADR-007)

**Module:** `src/domain/ports/deployer.rs`, `src/application/context.rs`,
`src/application/use_cases/{deploy_fleet,rollback,detect_drift,exec_fleet}.rs`,
`src/infrastructure/deployment/{ssh_cli_deployer,local_deployer}.rs`, and their tests.
**References:** ADR-007 (`docs/adr/007-effective-connection-profile.md`).
**Config Cascade:** unchanged (ADR-004).

## Problem

`SshCliDeployer` and `ExecFleetUseCase` re-derive a *primitive* connection profile via
`SshProfile::for_host(host)`, which only materializes `user`, `port`, and
`sudo = host.is_local`. Every other resolved setting — `identity_file`, `proxy_jump`,
`proxy_command`, `timeout_secs`, `connect_timeout_secs`, `extra_ssh_args`,
`allow_insecure` — is silently discarded, so SSH hosts configured with a non-default
port, an identity file, or a jump host are connected to as `root@target:22` with none of
them applied. This is silent configuration loss (audit BLOCKER B1).

## Decision (from ADR-007)

The resolved `SshProfile` is an explicit argument to the deploy port. The caller (a use
case or command) obtains it via a single resolver; adapters become dumb transports that
consume only the passed profile and never call `SshProfile::for_host` internally.

## Acceptance Criteria

### AC1 — Resolver is the single source of the effective profile
`AppContext` exposes:

```rust
async fn resolved_profile(&self, host: &HostEntity) -> Result<SshProfile, NodError>
```

Behaviour:
- If a `ConfigStorePort` is bound, it returns `config_store.resolve(host)` — the full
  resolved profile (ADR-004 four-tier cascade).
- If no `ConfigStorePort` is bound, it falls back to `SshProfile::for_host(host)` and
  the method documents that this is a *primitive/unresolved* profile for dependency-free
  contexts (tests, `ssh`/`exec` when no config store is wired). The fallback is
  deliberate and documented, not silent widening.

Test: with a bound config store, `resolved_profile(h)` equals the store's `resolve(h)`;
without one, it equals `SshProfile::for_host(h)`.

### AC2 — Deploy port carries the effective profile
`DeployerPort` signatures change so the profile is passed in, never re-derived:

```rust
#[async_trait]
pub trait DeployerPort: Send + Sync {
    async fn check_reachability(&self, host: &HostEntity) -> Result<bool, NodError>; // unchanged: no profile needed
    async fn current_closure(&self, host: &HostEntity, profile: &SshProfile) -> Result<Option<PathBuf>, NodError>;
    async fn deploy_and_activate(
        &self,
        host: &HostEntity,
        profile: &SshProfile,
        closure: &Path,
        action: &str,
        verbose: bool,
    ) -> Result<(), NodError>;
    async fn rollback(&self, host: &HostEntity, profile: &SshProfile) -> Result<(), NodError>;
}
```

`check_reachability` stays `&HostEntity` (it only pings `target_host` and needs no
credentials).

### AC3 — SshCliDeployer honours every resolved knob
`SshCliDeployer` removes all internal `SshProfile::for_host(host)` calls and instead
uses the passed `profile` to build the SSH argv for `current_closure`,
`deploy_and_activate`, and `rollback`:

- `ssh_target = format!("{}@{}", profile.user(), host.target_host)` — flag-adjusted:
  - `-p <port>` when `profile.port() != 22`
  - `-i <identity_file>` when present
  - `-o ProxyJump=<jump>` when present (and/or `-o ProxyCommand=...`)
  - `-o ConnectTimeout=<connect_timeout_secs>` (and `ServerAliveInterval`
    from `timeout_secs` if already expressed) — apply `extra_ssh_args` verbatim
- The `nix copy --to ssh://...` store target string uses `user@host:port` (append
  `:port` only when `!= 22`) and the same identity/proxy/extra-args as `ssh`.

Reuse the existing pure, unit-tested `build_ssh_args` shape already present in
`commands/ssh.rs` — extract it to a shared dependency-free module so both the `ssh`
command and the SSH transport consume the same argument knowledge (DRY; closes warning
W8 for the argument builder itself). No adapter re-derives its own profile.

### AC4 — LocalDeployer unchanged in behaviour
`LocalDeployer` takes `&SshProfile` in the updated signatures but continues to ignore it
(its transport is `sudo <switch_bin> <action>` on the local host); the parameter is
underscore-prefixed (`_profile`) and documented. No behaviour change.

### AC5 — All callers go through the resolver
Every production call to `current_closure`, `deploy_and_activate`, and `rollback`
(`DetectDriftUseCase`, `DeployFleetUseCase::run_host`/`end_host`, `RollbackUseCase`)
resolves the profile first via `AppContext::resolved_profile(&host)` and passes it.
No caller uses `SshProfile::for_host` to feed a transport.

### AC6 — Exec fleet uses the resolver too
`ExecFleetUseCase` replaces its direct `SshProfile::for_host(&host)` with
`resolved_profile(&host)` before calling the shared `build_ssh_args`, so an SSH `exec`
honours identity/proxy/port like every other SSH path. (Build args come from the single
shared module.)

### AC7 — Regression test proves the contract
A test asserts that the profile actually reaching the SSH transport equals the
config-store-resolved profile: `resolve(h)` (from a bound store) is what
`resolved_profile(h)` returns and what `build_ssh_args`/svcagv diagnostics consume. The
existing adapter and use-case mock tests (`expect_deploy_and_activate`, etc.) are
updated for the new signature and still pass.

## Out of Scope
- No new `SshProfile` fields; shape frozen (ADR-007).
- Dashboard action wiring, rollback fleet semantics, audit error propagation, and
  generation flag are separate waves (follow this one).
- `HostEntity::ssh_profile()` may be removed if it is now unused outside the fallback;
  if kept, it must be documented as the primitive (non-resolved) profile.

## Verification
- `cargo test` — all pass (existing 186 + new tests).
- `cargo clippy --all-targets -- -D warnings` — zero warnings.
- `cargo fmt --check`.
- No `SshProfile::for_host` call remains inside `src/infrastructure/` (grep check).
