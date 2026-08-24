# Spec: Adapter Hardening (P5, W4-W7)

**Module:** `src/infrastructure/quality_gate.rs`,
`src/infrastructure/health/systemd_checker.rs`,
`src/infrastructure/deployment/ssh_cli_deployer.rs`,
`src/infrastructure/nix/cli_evaluator.rs`, `src/domain/errors.rs`.
**References:** ADR-002 (typed errors), ADR-004.
**Depends on:** P1-P4 in place. `NodError::health_check(...)` constructor exists (AC4/P4).

## Problem (verified facts)

1. **Quality gate (W4):** `quality_gate::run_all` returns `anyhow::Result` (not
   `NodError`), and each gate uses `if let Ok(status)` which swallows launch errors. The
   hard gate `deadnix` therefore **passes when deadnix is not installed** and `run_all`
   still prints "✓ Passed". nixfmt/statix are intentionally warn-only, but a missing tool
   should be a visible warning, not a silent success.
2. **Systemd checker (W5):** `failed_units` parses on the `●` U+25CF glyph, which is
   unreliable when systemd disables color on a pipe. `verify_health` never reads
   `host.nod_config.health_checks` — the resolved config (required_units, tcp, http,
   custom, enable) is inert.
3. **SSH remote command (W6):** `deploy_and_activate` builds
   `format!("{} {}", switch_bin.display(), action)` and passes it as a single string to
   `ssh`, which shells it via the remote shell. `action` is never validated and the path
   is never quoted → injection/breakage vector. Rollback uses a hardcoded string (safe).
4. **Per-host eval fallback (W7):** `cli_evaluator` silently swallows three layers
   (spawn error → JSON parse error → `FlakeMeta::default`), dropping every `nod` config
   field, and it interpolates `flake_path.display()`/`name` into Nix source without
   quoting.

## Decision & Acceptance Criteria

### AC1 — Quality gate: typed errors + honest missing-tool handling
- `QualityGate::run_all(flake_path) -> Result<(), NodError>` (drop `anyhow`).
- For the **hard** gate (`deadnix`): a launch failure (binary missing / process failed to
  start) is a hard failure → `Err(NodError::config("deadnix check could not run: ..."))`.
- For the **warn-only** gates (nixfmt, statix): a launch failure prints an explicit
  visible warning ("⚠ <tool> not found; skipping <tool> check"), not silent success.
- A non-zero exit on nixfmt/statix remains warn-only (unchanged intent); a non-zero exit
  on deadnix remains a hard error (unchanged intent).
- Tests: a missing/launch-failing deadnix yields an `Err`; a launch-failing nixfmt yields
  `Ok` with the warning (no hard failure). Implement via an injectable command-spawn seam
  where feasible; otherwise test the pure decision given a `Result<ExitStatus>` input.

### AC2 — Systemd checker: column-based parsing + honour enable/required_units
- `failed_units(output)` parses the systemd `--failed` table by the `UNIT` column and the
  `failed`/`failed` state columns, not the `●` glyph. The path that previously matched
  lines containing `●` is replaced with a line-split where a unit is included if its
  status column indicates a failed state.
- `verify_health(host)`:
  - Reads `host.nod_config.health_checks.enable`; if `enable == Some(false)`, health
    verification is skipped and the host is considered healthy (documented).
  - Reads `host.nod_config.health_checks.required_units` (the resolved list from the
    config store) and fails (returns `false`) if any required unit is not active. If
    `required_units` is empty/None, it falls back to the current "no failed units +
    system running" semantics (backward compatible).
  - tcp/http/custom probes are **out of scope** (documented future work, keep the existing
    structural path for them).
- Tests: table with real systemd column output (no `●`) parses the failed unit;
  `enable=false` short-circuits healthy; a required unit that is failed/inactive yields
  `false`.

### AC3 — SSH remote command quoting + action validation
- Validate `action` against an allow-list of the known valid deployment actions
  (`switch`, `boot`, `test`, `dry-run`, `build` per `DeploymentAction`). An unknown action is
  rejected with a `NodError::config` at the command boundary before any SSH runs
  (defense-in-depth; the CLI already constrains it via clap ValueEnum where applicable).
- Shell-quote the `switch_bin` path (`bin/switch-to-configuration`) when embedding into
  the remote command string, so a path containing spaces/special characters survives the
  remote `/bin/sh -c`.
- Prefer passing a single word `action` (allow-listed, so no shell metacharacters) and a
  single-quoted path. Rollback's hardcoded string is unchanged (already safe).
- Add a test that an action outside the allow-list is rejected, and that a path with
  spaces produces a correctly quoted remote command.

### AC4 — Per-host eval: propagate failures (no silent default), quote interpolation
- Remove the silent three-layer `unwrap_or_else(|| FlakeMeta::default)` for per-host
  metadata. A failure to spawn `nix eval` or to parse its JSON output **propagates** as a
  `NodError::evaluation` (or `parse_failure`) with the host name, so discovery no longer
  silently reports success with a wrong target host / dropped `nod` config.
- The fallback `target_host = name` path is removed for the error case; the host is now
  reported as failed to evaluate rather than connected to the wrong target.
- Quote/escape `flake_path` and `name` when interpolated into the `meta_expr` Nix source
  (Nix string escaping), so paths/host names with special characters evaluate correctly.
- Tests: a failing `nix eval` yields an `Err` (not a defaulted `FlakeMeta`); a
  path/name containing a quote or space is escaped so the generated expression is valid.

## Out of Scope
- Full tcp/http/custom health probes (documented in AC2 as future work).
- Any change to the four-tier config cascade (ADR-004).
- P6 (docs/ADR finalization) — separate.

## Verification
- `cargo test` — all pass (existing + new AC1-AC4 tests).
- `cargo clippy --all-targets -- -D warnings` — zero warnings.
- `cargo fmt --check`.
- `grep -rn "anyhow" src/infrastructure/quality_gate.rs` → none (AC1).
- `grep -rn "●" src/` → none in systemd_checker (AC2).
- The `health_check` NodError constructor is used consistently (AC4, P4).
