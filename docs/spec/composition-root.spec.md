# Spec: Centralized Composition Root & Target Resolution (ADR-008)

**Module:** `src/application/context.rs`, `src/application/selection.rs`, `src/main.rs`,
`src/commands/*.rs`.
**References:** ADR-001 (hexagonal / AppContext as single wiring point), ADR-006
(target selection), ADR-007 (resolved profile).
**Depends on:** P1 (B1-B4) already in place: `AppContext::resolved_profile` exists and
all transport paths honour it.

## Problem

Two structural defects surfaced by the audit and review:

1. **No single composition root.** `AppContext::new(Arc::new(NixCliEvaluator::new()),
   Arc::new(LocalDeployer::new()), Arc::new(SshCliDeployer::new()))` (with a
   `TomlConfigStore::new(...)` + `.with_config_store(...)`) is copy-pasted verbatim in 8
   command files, plus twice in `main.rs` for `ssh`/`exec`. There is no one place that
   defines how the production graph is wired (ADR-001: "AppContext is the only injection
   point"). Because `status` and `dashboard` build bare contexts and `ssh`/`exec`
   receive a bare context with `flake=None`, several commands never bind a
   `ConfigStorePort` — so `AppContext::resolved_profile` (added in ADR-007) falls back to
   the primitive `SshProfile::for_host` at runtime for `ssh`/`exec`, silently discarding
   configured `identity_file`/`proxy_jump`/port (the Welle-1 review warning).

2. **Target selection pipeline is duplicated and has diverged.** The
   "discover → local hostname → compute effective target → select" block is repeated 9
   times. The 8 deploy commands default to `(Some("local"), false)`; `status.rs:36`
   defaults to `all_effective = all || (...)` → ALL hosts. Same-shaped code, opposite
   defaults — an inconsistency that is a latent bug factory.

## Decision

### 1. A single production factory (ADR-008, Composition Root)
`AppContext::production(flake_path: &Path, cli_overrides: CliOverrides) -> Result<AppContext, NodError>`
builds the complete production graph in **one** place:

- evaluator = `Arc::new(NixCliEvaluator::new())`
- local deployer = `Arc::new(LocalDeployer::new())`
- ssh deployer = `Arc::new(SshCliDeployer::new())`
- config store = `Arc::new(TomlConfigStore::new(flake_path, cli_overrides)?)`
- audit store = NOT bound by default (only `audit` uses it; see below)

`main.rs` becomes the **only** composition root: it calls `AppContext::production(...)`
for every command and passes the `ctx` into `execute`. The 8 command files stop
constructing `AppContext`/`TomlConfigStore` themselves and instead accept `ctx` as a
parameter.

Consequence for `ssh`/`exec`: they now receive a `production`-built context **with** a
config store, fixing the Welle-1 review warning (resolved connection settings now apply
to `nod ssh` / `nod exec`). `ssh` currently receives `flake=None`; give it a flake path
(main defaults it to `"."`, consistent with every other command) so the store can be
built.

`audit` additionally needs an `AuditStorePort`. Keep that explicit: `audit` calls
`ctx.with_audit_store(Arc::new(JsonAuditStore::new()))` on the `production` ctx (a single
site, not 8 copies). Alternatively `production` accepts an optional audit-store flag; the
brief is that the *base graph* is centralized — optional services remain opt-in per
command at a single call site.

### 2. Centralized target resolution (ADR-008, closes the divergence)
Add to `src/application/selection.rs`:

```rust
/// Which host set to target when no target/tag/role and `!all` is given.
pub enum DefaultScope { Local, All }

pub fn resolve_targets(
    hosts: Vec<HostEntity>,
    local_hostname: &str,
    target: Option<&str>,
    tag: Option<&str>,
    role: Option<&str>,
    all: bool,
    default_scope: DefaultScope,
) -> Vec<HostEntity>
```

Behaviour:
- If `!all && target.is_none() && tag.is_none() && role.is_none()`:
  - `DefaultScope::Local` → select `("local", false)`
  - `DefaultScope::All` → select `(None, true)` (all hosts)
- Otherwise → select the given filters unchanged.
- Delegates to the existing `TargetSelection::select` (and the shared `discovery` caller
  passes `flake_path`/`verbose` in the command, since `resolve_targets` is pure selection).

Every command calls `resolve_targets` instead of hand-rolling the block. Deploy commands
use `DefaultScope::Local`; `status` uses `DefaultScope::All`. The *decision* is now
documented in one enum rather than scattered.

### 3. Shared fleet summary rendering
Move the identical `render_summary` into one function, e.g.
`commands::render_summary(summary, verbose)` (or a method on `FleetSummary` in
`application`). `build.rs` renders per-outcome failure detail; the shared function
renders it whenever present (the more informative form) so all four commands are
identical and there is no divergent third copy.

## Acceptance Criteria

- **AC1 — One composition root.** `AppContext::production(flake, overrides)` exists and
  is the only place that constructs the evaluator/deployers/config-store graph. `grep`
  shows `AppContext::new(` no longer appears inside any `src/commands/*.rs` (only
  `context.rs` internal + tests).
- **AC2 — main is the root.** `main.rs` calls `production(...)` for every command arm and
  passes `ctx` to `execute`. No command constructs `AppContext` or `TomlConfigStore`
  itself.
- **AC3 — ssh/exec bind a config store.** After the change, `nod ssh` and `nod exec`
  reach `resolved_profile` with a `Some(config_store)`, so configured
  identity/proxy/port are honoured (Welle-1 warning closed). `ssh` receives a flake path
  (defaulted to `"."` in main) to enable this.
- **AC4 — Target resolution centralized.** `resolve_targets` + `DefaultScope` exist in
  `selection.rs`. Every command uses it. `status` passes `DefaultScope::All`; deploy and
  exec commands pass `DefaultScope::Local`. No command hand-rolls the effective-target
  block. `select_exact_one` usage (rollback/ssh) is preserved via
  `resolve_targets(...)` followed by the exact-one helper where the command needs a
  single host.
- **AC5 — Shared summary renderer.** One `render_summary` implementation used by
  switch/test/boot/build; behaviour includes per-outcome failure detail. No duplicate.
- **AC6 — No behaviour regression.** Test evidence: all existing command behaviours hold
  (a single deploy path with no flags still targets local; `status` with no flags still
  lists all hosts). Add a focused `selection` test for `DefaultScope::Local` vs
  `DefaultScope::All` with identical empty inputs (the exact divergence that existed).
- **AC7 — Honest dashboard actions.** `nod dashboard` action keys (`s`/`r`/`d`) do **not**
  silently claim a live switch/rollback/diff. Because wiring the dashboard event loop to
  real deploy dispatch is a separate cross-cutting change, this wave (minimal, honest)
  relabels the footer/keys so the actions are shown as **preview/log** intents, and leaves
  `run_action` as an explicit, documented future wiring point (a `TODO` referencing the
  deferred dashboard-deploy feature, + roadmap note). No key is presented as a live
  state-changing operation it does not perform.

## Out of Scope
- Real dashboard action dispatch (deploy triggered from the TUI event loop) — deferred
  and tracked (see AC7 relabel only).
- SSH-field consolidation (P3), application/port cleanup (P4), adapter hardening (P5) —
  separate waves.
- No change to the four-tier config cascade (ADR-004).

## Verification
- `cargo test` — all existing tests + new `DefaultScope`/resolution tests pass.
- `cargo clippy --all-targets -- -D warnings` — zero warnings.
- `cargo fmt --check`.
- `grep -rn "AppContext::new(" src/commands/` → no matches (AC1).
- `grep -rn "TomlConfigStore::new" src/commands/` → no matches (AC2).
- `grep -rn "fn render_summary" src/commands/` → exactly one definition (AC5).
