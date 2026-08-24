# ADR-008: Single Composition Root & Centralized Target Resolution

- **Status:** Accepted
- **Date:** nod v2 hardening
- **Deciders:** nod maintainers
- **Supersedes:** n/a (tightens ADR-001's wiring rule and ADR-006's selection idiom)

## Context

ADR-001 mandates "AppContext is the only injection point; command wiring reaches
Infrastructure only through resolved ports." In practice the composition is scattered:
the identical
`AppContext::new(Arc::new(NixCliEvaluator::new()), Arc::new(LocalDeployer::new()),
Arc::new(SshCliDeployer::new()))` block (plus `TomlConfigStore::new(...)` +
`.with_config_store(...)`) is duplicated in 8 command files and twice more in `main.rs`.

Two concrete consequences:

1. **Inert resolution plumbing.** `status`/`dashboard` build bare contexts and `ssh`/
   `exec` receive a context with `flake=None`, so no `ConfigStorePort` is ever bound on
   those paths. ADR-007 added `AppContext::resolved_profile`, which therefore *always*
   falls back to the primitive `SshProfile::for_host` for `nod ssh` and `nod exec`,
   silently discarding configured `identity_file`/`proxy_jump`/port. The feature ADR-007
   guarantees is inert at runtime for two commands.
2. **Target-selection drift.** The "discover → local hostname → effective target →
   select" pipeline is repeated 9×. Eight deploy commands default to local; `status`
   defaults to _all_. Same-shaped code, opposite defaults.

## Decision

### 1. `AppContext::production()` — the single composition root

`main.rs` is the only composition root. It calls a single factory per command arm and
passes the context to `execute`:

```rust
AppContext::production(flake_path: &Path, cli_overrides: CliOverrides)
    -> Result<AppContext, NodError>
```

The factory—and only the factory—constructs the evaluator, both deployers, and the
`TomlConfigStore` (bound as the `ConfigStorePort`). Commands stop constructing `AppContext`
or `TomlConfigStore`; they accept `ctx: AppContext` and resolve optional services (e.g.
`audit`'s `AuditStorePort`) at a single, explicit call site (`ctx.with_audit_store(...)`).

Consequence: `ssh` and `exec` now bind a config store (ssh receives a flake path,
defaulted to `"."` like every other command), so resolved connection settings are
honoured on those paths — closing the inert-resolution gap.

### 2. `resolve_targets` + `DefaultScope` — one target idiom

The selection pipeline is centralized in `application/selection.rs`:

```rust
pub enum DefaultScope { Local, All }
pub fn resolve_targets(hosts, local_hostname, target, tag, role, all, default_scope) -> Vec<HostEntity>
```

The default-when-empty intent is now an explicit, documented enum value at one call site
per command, instead of an implicit copy-pasted rule that diverged. Deploy/exec use
`DefaultScope::Local`; `status` uses `DefaultScope::All`. `select_exact_one` remains the
single-host gate for `rollback`/`ssh`, applied after `resolve_targets`.

### 3. Shared summary renderer

The byte-identical `render_summary` in `switch`/`test`/`boot` and the `build` variant
(extra failure line) collapse into one function that always renders per-outcome failure
detail when present.

## Consequences

- Wiring changes in one place (add a port/adapter → edit `production`), not 10.
- `ssh`/`exec` correctly apply resolved SSH settings, closing the ADR-007 runtime gap.
- The target default is a reviewed, singular decision rather than nine silent copies.
- `main.rs` grows as the composition root; command files thin out (presentation-only).

## Alternatives

- **Leave wiring per-command; fix only `ssh`/`exec` in place.** Rejected: leaves the
  duplication that ADR-001 warns against and invites the next command to copy the next
  variant.
- **Auto-resolve inside the transport adapters (inject the store).** Rejected in
  ADR-007: binds transport to config (SRP).
- **Keep `DefaultScope` implicit via a bare `bool`.** Rejected: a bare bool that means
  "local" in one command and "all" in another is exactly the divergence we are removing.
