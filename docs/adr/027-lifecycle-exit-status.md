# ADR-027: Lifecycle Exit Status Reflects Host Outcomes

- **Status:** Accepted
- **Date:** nod v2.2 hardening
- **Deciders:** nod maintainers
- **Supersedes:** n/a (completes the reporting contract of ADR-010 and ADR-013)

## Context

`nod switch`, `test`, `boot` and `build` run a fleet rollout through
`DeployFleetUseCase`, render the per-host outcome with `render_summary`, and
returned `Ok(())` — including when a host ended `Failed` or `RolledBack`. The
operator saw the failure in the output and it was recorded in the audit log, but
the process exit status was 0, so CI and scripts treated a failed deployment as a
success. A target-selection error exited 1; a failed host did not, which is
inconsistent for automation.

## Decision

Route every lifecycle command's summary through one presentation helper,
`commands::report_summary(&FleetSummary, quiet) -> Result<(), NodError>`. It
renders the summary (unless `--quiet`) and returns `NodError::Deployment`, naming
how many hosts did not complete, when any outcome has `ok == false`. `main`
already maps a returned `NodError` to a non-zero exit, so the exit status becomes
a faithful statement about the fleet.

An outcome is successful when it reached a good terminal state (`Completed`, or
`Prepared` for a dry-run preview). A `RolledBack` host counts as unsuccessful:
the deployment did not take effect, whether or not the rollback itself succeeded.
`--on-error continue` still does not abort the remaining waves, but it no longer
masks the failure in the exit status.

## Consequences

- A red deployment is red in CI without parsing stdout; `nod switch && …` is safe.
- The rule lives in one function used by all four lifecycle commands
  (`switch`/`test`/`boot`/`build`), so `switch` and `build` cannot diverge.
- `--quiet` suppresses the rendering, never the status.
- Scripts that intentionally tolerate failures must branch on the exit status
  explicitly (`|| true`) instead of relying on a silent zero.
