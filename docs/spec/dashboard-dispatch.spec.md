# Spec: Real Dashboard Action Dispatch (P7b)

**Module:** `src/ui/mod.rs`, `src/ui/app.rs`, `src/ui/views.rs`.
**References:** ADR-008 (AC7 labelled the keys as preview/log-only via `TODO(dashboard-deploy)`),
ADR-003, ADR-005.
**Depends on:** P1-P6, P7c, P7a in place. `AppContext::production` and the three use cases
exist.

## Problem (verified)

The dashboard's `s`/`r`/`d` action keys (`Switch`/`Rollback`/`Diff`) are **preview/log-only**:
`run_action` (src/ui/mod.rs:100-117) only calls `app.append_log(...)`, with a
`TODO(dashboard-deploy)` noting the deferral (ADR-008 AC7). The keys are honestly labelled
as preview — but the feature they are meant to provide (drive a deploy from the TUI) is not
wired.

Root cause: `run_dashboard(ctx, ...)` takes `AppContext` by value, uses it only for
startup discovery + reachability, then **drops it before the event loop** (`ctx` is not part
of `DashboardApp`; `drive_events`/`run_action` never receive it). So no use case can be
invoked from a keypress.

All three target use cases support single-host operation (verified):
- `DeployFleetUseCase::execute(Vec<HostEntity>, DeploymentOptions, flake_path)` — pass a
  `vec![selected_host]`.
- `RollbackUseCase::execute(&HostEntity)` — single host by design.
- `DetectDriftUseCase::execute(&HostEntity, flake_path, verbose)` — single host by design.

`app.selected_host()` returns `Option<&HostEntity>` — exactly what the single-host use cases
consume.

## Decision

Wire real dispatch from the TUI event loop. Keep the action **explicit and honest** in the
UI (a keypress performs a real, state-changing operation — the footer and status message
must make that unmistakable, not a silent side effect).

### Design

1. **`run_dashboard` takes `Arc<AppContext>`** (change `AppContext` by value → `Arc`), so the
   context can be handed into the event loop. (`main.rs` already constructs via
   `AppContext::production`; pass `Arc::new(ctx)`.)
2. **Thread the context through the event loop**: `drive_events(&mut app, &mut terminal,
   flake_path, ctx_arc)` and `run_action(action, app, flake_path, ctx_arc, verbose)`.
3. **`run_action` dispatches** (async, since use cases are async):
   - `Switch` → `DeployFleetUseCase::new(ctx_arc.clone())`
     `.execute(vec![selected_host.clone()], DeploymentOptions::default_for(DeploymentAction::Switch), flake_path)`.
   - `Rollback` → `RollbackUseCase::new(ctx_arc.clone()).execute(&selected_host)`.
   - `Diff` → `DetectDriftUseCase::new(ctx_arc.clone()).execute(&selected_host, flake_path, false)`.
   - Each result is reported into the dashboard log and/or status message: success →
     `"<host>: <outcome>"`, failure → `"<host>: error: <err>"`, so the operator sees the
     result of the action they triggered. The `selected_host` is taken from
     `app.selected_host()`. Guard: if no host is selected (`None`), log a warning and
     return (no action).
4. **Honest UI labelling** (ADR-008 AC7 reversal now that they are live): update the footer
   in `views.rs` and the `handle_key` status messages from "preview switch/rollback/diff" /
   "switch queued" to reflect that `s`/`r`/`d` now perform a real deploy/rollback/diff on
   the selected host. Remove the `TODO(dashboard-deploy)` and the "PREVIEW/LOG-ONLY"
   comment in `run_action`; replace with dispatch documentation. Keep the roadmap note
   updated (dashboard actions are now live).

### Concurrency note
The dashboard event loop is single-threaded (ratatui + crossterm). A long deploy blocks the
UI. For this wave, run the use case **inline** (await) so the result is deterministic and
correct; document that a long-running deploy blocks the TUI until it completes (acceptable
for v2; a background-task render is future work and out of scope). The existing confirm/dry
semantics are unchanged (no new confirmation prompt required; the keypress is the
intent — but the status message must make the action obvious before it runs).

## Acceptance Criteria

- **AC1** — `run_dashboard` takes `Arc<AppContext>`; the event loop and `run_action`
  receive the context.
- **AC2** — `s`/`r`/`d` dispatch real single-host use-case calls (deploy/rollback/diff) on
  the selected host with the flake path, and the outcome (success or error) is surfaced in
  the dashboard log/status.
- **AC3** — No host selected → a warning is logged and no use case is invoked (no panic, no
  silent no-op).
- **AC4** — Footer and key status messages no longer say "preview"; they state that
  `s`/`r`/`d` perform a live switch/rollback/diff on the selected host. The
  `TODO(dashboard-deploy)` and "PREVIEW/LOG-ONLY" comment are removed/updated.
- **AC5** — `main.rs` passes `Arc::new(ctx)` to `run_dashboard`; the app compiles and the
  dashboard test harness (headless `TestBackend`, `ui::app::tests`) still passes.
- **AC6** — Tests render the updated footer (no "preview"), and the dispatch wiring is
  covered by the existing app/views tests plus any feasible unit seam (keep `handle_key`
  returning `DashboardAction`; run_action may be unity-seam-tested for the no-host guard).
- **AC7** — Behaviour note: no `Notify`-style background task added; deploy blocks the UI
  inline (documented).

## Out of Scope
- Background/async rendering of a running deploy (documented as future work).
- Confirmation prompts / dry-run toggles in the UI.
- P7a (discovery degradation — separate), P7d (field consolidation) — separate.
- Any change to use-case logic (they already support single host).

## Verification
- `cargo test` — all pass (existing + AC4/AC6 updates).
- `cargo clippy --all-targets -- -D warnings` — zero warnings.
- `cargo fmt --check`.
- `grep -rn "preview" src/ui/` → no remaining "preview switch/rollback/diff" label (AC4).
- `grep -rn "TODO(dashboard-deploy)" src/ui/` → none (AC4).
