# nod — Observability & Audit Specification

> Gherkin feature specification for post-deployment health checking, closure
> drift detection and audit logging (ADR-003 `003-deployment-state-machine.md`).
> Each `Feature` is behavior-observable against **mock adapters** — no real
> Nix/SSH/systemd/network is required.

---

## Feature: Post-Deployment Health Verification

```gherkin
@health @systemd
Feature: A deployed host passes or fails post-activation health probes

  Scenario: a running system without failed units is healthy
    Given a local host being verified after activation
    When the health checker probes `systemctl is-system-running`
      And the reported state is `running`
    When the health checker probes `systemctl --failed`
      And no failed units are listed
    Then `verify_health` reports the host is healthy

  Scenario: failed systemd units mark a host unhealthy
    Given a local host being verified after activation
    When `systemctl is-system-running` reports state `degraded`
      And `systemctl --failed` lists at least one failed unit
    Then `verify_health` reports the host is unhealthy

  Scenario: a host whose system never started is unhealthy
    Given a local host being verified after activation
    When `systemctl is-system-running` reports a non-running state
    Then `verify_health` reports the host is unhealthy

  Scenario: the pipeline refuses to complete an unhealthy deployment
    Given a host in the `Verifying` stage
    When the health probe returns unhealthy
    Then the pipeline does not return `Completed`
    And the host enters rollback recovery

  Scenario: a remote host cannot be verified by the local systemd adapter
    Given a remote host
    When the local systemd health checker is asked to verify it
    Then a typed health-check error is raised
    And no systemctl command runs on the remote host
```

---

## Feature: Closure Drift Detection (`nod drift`)

```gherkin
@drift @closure
Feature: A host exposes drift when its live closure differs from the flake

  Scenario: active and flake closures match means no drift
    Given a host whose live closure resolves to the same store path as its
      freshly built flake closure
    When `DetectDriftUseCase` compares them
    Then the host is reported as not drifted

  Scenario: differing closures are flagged as drifted
    Given a host whose live closure resolves to a different store path than
      its freshly built flake closure
    When `DetectDriftUseCase` compares them
    Then the host is reported as drifted
    And both the live and flake paths are recorded in the report

  Scenario: a host with no live closure is treated as drifted
    Given a host that reports no active closure
    When `DetectDriftUseCase` compares it against its flake closure
    Then the host is reported as drifted

  Scenario: the active closure is resolved through the host deployer
    Given a local host
    When drift detection resolves the live closure
    Then the local deployer reads the active system profile (`/run/current-system`)
    When the host is remote
    Then the SSH deployer reads the live closure over the transport
```

---

## Feature: Audit Logging (`nod history`)

```gherkin
@history @audit
Feature: `nod history` reads back recorded deployment outcomes

  Scenario: a deployment outcome is recorded
    Given a history store bound to a writable path
    When `HistoryStorePort::record` is called for a host with outcome `completed`
    Then the entry is appended to the store

  Scenario: history lists recorded entries newest-first
    Given a history store with several recorded outcomes
    When `HistoryStorePort::entries` is called without filters
    Then every recorded entry is returned, newest first

  Scenario: history narrows by target host
    Given a history store with outcomes for hosts `jello` and `atlas`
    When `entries` is called with host filter `jello`
    Then only `jello` entries are returned

  Scenario: history caps the returned count
    Given a history store with more than `limit` entries
    When `entries` is called with `limit` set
    Then at most `limit` newest entries are returned

  Scenario: missing history reads as empty, not an error
    Given a history store whose backing file does not exist
    When `entries` is called
    Then an empty history is returned (no error)
```

---

> **Conventions:** the health probe maps to `HealthCheckUseCase` over the
> `HealthCheckerPort::verify_health` seam; drift maps to `DetectDriftUseCase`
> over `EvaluatorPort::build_toplevel` + `DeployerPort::current_closure`; the
> audit log maps to `AuditLogUseCase` over `HistoryStorePort::entries`.
> Tag names (`@drift`, `@history`, `@health`) are stable anchors for the runner.