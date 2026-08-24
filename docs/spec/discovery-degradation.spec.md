# Spec: Contingent Fleet-Discovery Degradation (P7a)

**Module:** `src/infrastructure/nix/cli_evaluator.rs`.
**References:** ADR-002, ADR-006.
**Depends on:** P1-P6, P7c in place.

## Problem (verified)

`discover_hosts` hard-aborts on the **first** per-host `nix eval` failure
(`cli_evaluator.rs:174`, `let meta = Self::eval_meta(&name, meta_output)?;`). One broken
host config aborts discovery for the whole fleet — a sharp UX edge for large fleets
audit W7 follow-up. But a blanket "skip-and-return-partial" would be wrong for *targeted*
commands: `ssh`/`rollback` gate on `select_exact_one`, so silently skipping a failed
targeted host would surface as a confusing "no hosts matched" instead of the real error.

## Decision — contingent degradation

Discovery degrades **conditionally on how the caller consumes the result**, decided by
the command layer, not by changing `EvaluatorPort::discover_hosts`'s signature (keeps the
hexagonal boundary and the 12 call sites unchanged):

- **Fleet mode** (command targets many hosts: `--all`, a broad glob, or a bare fleet
  command): per-host eval failures are **skipped and reported** — the failing host is
  omitted from the returned `Vec<HostEntity>` and a clear warning is printed for it. The
  rest of the fleet deploys.
- **Targeted mode** (a single explicit host is requested): a per-host eval failure is a
  **hard error** propagated with the host name, so an operator is never silently
  disconnected from the one host they meant to touch.

### Implementation

To give the command layer the decision without changing the trait signature, expose two
functions on `CliEvaluator` (or the evaluator impl):

```rust
/// Discovers all reachable hosts; per-host metadata eval failures are skipped
/// (omitted from the result) and reported via a warning. Never hard-fails on a
/// single host's metadata; only the whole-matrix `nix eval` fails hard.
pub async fn discover_hosts_degraded(&self, flake_path: &Path, verbose: bool)
    -> Result<Vec<HostEntity>, NodError>

/// Discovers as today: any per-host metadata eval or parse failure is a hard
/// error carrying the failing host's name. Used by targeted single-host paths.
pub async fn discover_hosts_strict(&self, flake_path: &Path, verbose: bool)
    -> Result<Vec<HostEntity>, NodError>
```

- `discover_hosts_strict` = the existing body (rename/keep current `discover_hosts`).
- `discover_hosts_degraded` = same flow, but the per-host loop replaces the `?` on
  `eval_meta` with a `match`: on `Err(e)`, `eprintln!("warning: skipping host '{name}':
  {e}")` and `continue`; on `Ok`, push the entity.

(Alternative considered: change `EvaluatorPort::discover_hosts` to return a
`DiscoverResult { hosts, errors }`. Rejected — it ripples through 12 call sites + 3 mock
implementations, and the error-capacity type `NodError` has no list form. Adding a domain
type for this one degradation is over-engineering for the current value. The two-function
split keeps the port stable and the decision at the command seam.)

### Command disposition (the contingent rule)

Command layer chooses the strict or degraded variant:

- **Targeted single-host** (`ssh`, `rollback`, and a `switch`/`diff`/`plan`/`boot`/
  `build`/`test`/`drift`/`exec`/`status` invoked with an explicit positional `target`
  meaning exactly one host): use `discover_hosts_strict` → a targeted host that fails
  eval reports the real host error, not a confusing empty match.
- **Fleet/all** (`--all`, a glob matching many, or bare fleet default): use
  `discover_hosts_degraded` → the broken host is reported-and-skipped, the rest proceeds.
- `status`: degraded (status should show what it can, and a broken host showing as absent
  with a warning is acceptable — status is read-mostly).

The exact selection of variant per command is decided where the command knows its
intent (after target/filter parsing).

## Acceptance Criteria

- **AC1** — Two functions exist (`discover_hosts_degraded`, `discover_hosts_strict`);
  the port signature (`EvaluatorPort::discover_hosts`) is unchanged.
- **AC2** — `discover_hosts_degraded` prints `warning: skipping host '<name>': <err>` and
  omits the failing host while returning the successfully discovered hosts; it does not
  hard-fail a single host's metadata failure. A whole-matrix `nix eval` failure still
  hard-fails (unchanged).
- **AC3** — `discover_hosts_strict` fails hard on the first per-host failure with the
  host name (current `discover_hosts` behaviour, preserved).
- **AC4** — Targeted single-host commands (`ssh`, `rollback`) use the strict variant; the
  reaching `select_exact_one` error surface is unchanged for them. `--all`/fleet commands
  and `status` use the degraded variant with the warning.
- **AC5** — Tests: `discover_hosts_degraded` with one failing + several succeeding hosts
  returns the succeeding hosts and prints warnings (via an injectable seam; if a full
  subprocess test is impractical, factor the per-host loop into a testable
  `fn collect_hosts(flake_path, names, degraded: bool) -> impl Iterator<Item=Result<HostEntity, (String,NodError)>>` and unit-test that). Existing strict-path tests for
  `eval_meta` still pass; the strict function reuses them.
- **AC6** — No warning appears for the successfully discovered hosts; warning text names
  the host.

## Out of Scope
- Changing `EvaluatorPort::discover_hosts` return shape (kept).
- P7b dashboard dispatch, P7d field consolidation — separate.
- Reordering or parallelism of discovery.

## Verification
- `cargo test` — all pass (existing + AC2/AC4/AC5 tests).
- `cargo clippy --all-targets -- -D warnings` — zero warnings.
- `cargo fmt --check`.
- `grep -rn "discover_hosts_degraded\|discover_hosts_strict" src/` — both defined and
  wired in the command layer as intended (AC4).
