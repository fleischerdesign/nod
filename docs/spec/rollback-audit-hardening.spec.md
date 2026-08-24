# Spec: Audit Error Propagation, Rollback Fleet Semantics, Generation Flag (B2-B4)

**Module:** `src/infrastructure/storage/json_audit_store.rs`,
`src/application/use_cases/rollback.rs`, `src/commands/rollback.rs`, `src/main.rs`,
`src/config/options.rs`.
**References:** ADR-003 (observability), ADR-006 (target selection).
**Depends on:** Welle 1 (B1, ADR-007) already merged — its `AppContext::resolved_profile`
is available.

## B2 — Audit persistence must not silently lose outcomes

### Problem (audit BLOCKER B2)
`JsonAuditStore::append` swallows read errors (`if all.is_err() { all = Ok(Vec::new()) }`)
and write errors (`let _ = self.write(all)`), and `record` unconditionally returns
`Ok(())`. A disk-full / permission / IO error silently discards the deployment outcome
while the caller believes it was recorded. `AuditStorePort::record` returns
`Result<(), NodError>` precisely to let persistence failures propagate (ADR-003).

### Acceptance Criteria
- **AC-B2.1** — `append` returns `Result<(), NodError>`; read and write failures are
  propagated as `NodError::config` (or an `NodError` classification the caller can act
  on), never swallowed.
- **AC-B2.2** — `record` propagates the append error instead of unconditionally
  returning `Ok(())`.
- **AC-B2.3** — A corrupt existing history (unparseable JSON) is **not** silently
  truncated-and-rewritten. Corrupt history surfaces as an error (or is preserved), so
  prior records are not silently discarded. (Preference: fail the write with a clear
  message; do not overwrite a corrupt file with a single fresh entry.)
- **AC-B2.4** — Tests: (a) a successful append writes and reads back; (b) an
  unreadable/unwritable path yields an `Err` from `record` (not `Ok`); (c) corrupt input
  history yields an error rather than silent truncation. Existing `entries` behaviour
  (missing file = empty history) is unchanged.

## B3 — Rollback must not silently operate on a subset of the matched fleet

### Problem (audit BLOCKER B3)
`src/commands/rollback.rs:63` selects via `TargetSelection::select` (plural, honours
`--all`/tag/role) then unconditionally does `let host = targets[0].clone()` — so
`nod rollback --all` on a 5-host fleet silently rolls back exactly one host. Rolling back
is state-changing; silent truncation is a correctness defect.

### Decision
Rollback currently supports **exactly one host** (the `RollbackUseCase` and the SSH
deployer's `rollback` operate per-host via `nixos-rebuild --rollback switch` on one
target). Fleet rollback is not built; advertising fleet semantics while truncating is the
defect. The clean, minimal, honest fix is:
- Rollback accepts **exactly one** target (reject multi-match rather than silently
  truncating).
- The CLI help text and spec are corrected to state single-host semantics.

### Acceptance Criteria
- **AC-B3.1** — `rollback::execute` uses `TargetSelection::select_exact_one` (or
  equivalent that errors on 0 *and* on >1 matches) instead of `select` +
  `targets[0]`.
- **AC-B3.2** — Matching >1 hosts (e.g. `--all` on a fleet, or a glob matching several)
  yields a clear `NodError` ("rollback supports exactly one host target; got N" — or the
  existing not_found/config classification with an actionable message), never a silent
  `[0]` selection.
- **AC-B3.3** — `--all`/`--tag`/`--role` remain accepted in the CLI but their effect with
  rollback is single-target enforced by `select_exact_one`; the help text documents that
  rollback targets exactly one host. Update `docs/spec/lifecycle-commands.spec.md` and
  the `options.rs` doc comment to match.
- **AC-B3.4** — Tests: a single-match succeeds; a multi-match returns the documented
  error; an empty match returns the existing not-found error. Existing single-host tests
  still pass.

## B4 — Remove the inert `--generation` flag

### Problem (audit BLOCKER B4)
`main.rs:240` destructures `generation: _` (never forwarded), and `rollback::execute`
has no `generation` parameter. The help promises "Roll back to a specific prior
generation", but the flag is a silent no-op. `RollbackUseCase`/deployer do not model
generation selection at all.

### Decision
Choose **remove** over half-wiring, per the Minimality principle: the flag advertises a
feature that does not exist; keeping a documented-but-inert switch is worse than removing
it. Generation-targeted rollback is a real feature to build later (tracked separately),
at which point the flag returns with a working implementation.

### Acceptance Criteria
- **AC-B4.1** — The `generation` field is removed from `Commands::Rollback` in
  `src/config/options.rs`.
- **AC-B4.2** — The `main.rs` rollback arm no longer destructures or forwards a
  `generation`.
- **AC-B4.3** — `rollback::execute` signature is unchanged otherwise; update the
  `options.rs` rollback doc comment and any spec text that referenced `--generation`.
- **AC-B4.4** — Clap parsing tests are updated for the removed field.

## Out of Scope
- Fleet rollback (multi-host) — a future feature; B3 only prevents silent truncation.
- Generation-targeted rollback — a future feature; B4 only removes the inert flag.
- P2 wiring (central composition root), P3 SSH-field consolidation, P4/P5 cleanups —
  separate waves.

## Verification (all gates must pass)
- `cargo test` — all existing tests + new B2/B3/B4 tests pass.
- `cargo clippy --all-targets -- -D warnings` — zero warnings.
- `cargo fmt --check`.
- Review the B3 help/destructure and audit-store error paths manually with the quality
  gate (review subagent) before merge.
