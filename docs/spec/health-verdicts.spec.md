# nod — Health Verdict Specification

> Gherkin feature specification for ADR-029: what a deployment concludes about a
> host after activation, and when the fleet may call a revision verified. The
> scenarios are the decision rules in `src/domain/health.rs` and the probes in
> `src/infrastructure/health/probe.rs`; they describe observable behaviour, not
> an implementation.

---

## Feature: A host's post-activation verdict

```gherkin
@health @verdict @adr-029
Feature: A deployment concludes exactly one verdict per host

  Background:
    Given health checks are enabled for the host
    And a baseline snapshot was taken immediately before activation
    And a current snapshot is taken after activation
    And a snapshot records failed units, restarting units, probe problems and restart counters
```

### Healthy

```gherkin
  Scenario: A clean host is healthy
    Given the baseline has no problem
    And the current snapshot has no problem
    Then the verdict is Healthy
    And the host counts as converged

  Scenario: An improvement is healthy
    Given the baseline names a failed unit
    And the current snapshot has no problem
    Then the verdict is Healthy
    And the host counts as converged

  Scenario: A clean host without a baseline is healthy
    Given no baseline could be taken
    And the current snapshot has no problem
    Then the verdict is Healthy
    And the host counts as converged

  Scenario: A restart counter at the allowance is healthy
    Given the baseline records 1 restart for a declared unit
    And the current snapshot records 2 restarts for that unit
    And the host allows 1 restart increase
    Then the verdict is Healthy

  Scenario: An unchanged restart counter is healthy
    Given the baseline records 5 restarts for a declared unit
    And the current snapshot records 5 restarts for that unit
    Then the verdict is Healthy
```

### Degraded

```gherkin
  Scenario: A failure that appeared is a regression
    Given the baseline has no problem
    And the current snapshot names a failed unit
    Then the verdict is Degraded naming that unit
    And the verdict is a failure
    And the host does not count as converged

  Scenario: Only the regressed units are named
    Given the baseline names the unit old.service
    And the current snapshot names old.service and new.service
    Then the verdict is Degraded naming new.service only

  Scenario: Problems without a baseline are degraded
    Given no baseline could be taken
    And the current snapshot names a failed unit
    Then the verdict is Degraded naming that unit
    And the verdict is a failure

  Scenario: A restart loop is a problem even when the unit is not failed
    Given the baseline has no problem
    And the current snapshot names a unit in auto-restart
    Then the verdict is Degraded naming that unit

  Scenario: A restart counter above the allowance is a regression
    Given the baseline records 1 restart for a declared unit
    And the current snapshot records 2 restarts for that unit
    And the host allows 0 restart increases
    Then the verdict is Degraded naming that unit

  Scenario: A unit that does not exist is a problem
    Given the host declares a required unit
    And systemd reports LoadState not-found for that unit
    Then the current snapshot names "unit <name>: not found"

  Scenario: A unit that exited successfully is not a problem
    Given the host declares a required unit
    And systemd reports the unit inactive with Result success
    Then the current snapshot does not name that unit

  Scenario: A unit that stays activating is a problem
    Given the host declares a required unit
    And systemd reports the unit activating, not auto-restart
    And the unit is still activating after five seconds
    Then the current snapshot names "unit <name>: activating (<substate>)"

  Scenario: A closed TCP port is a problem
    Given the host declares a TCP port
    And nothing listens on that port
    Then the current snapshot names "tcp port <port>: not listening"

  Scenario: A missing ss is a problem, not a silent pass
    Given the host declares a TCP port
    And ss is not installed
    Then the current snapshot names "tcp: ss is not installed"

  Scenario: An HTTP probe must match the expected status
    Given the host declares an HTTP probe with an expected status
    And the endpoint answers with another status
    Then the current snapshot names "http <url>: expected <status>, got <status or none>"

  Scenario: A missing curl is a problem
    Given the host declares an HTTP probe
    And curl is not installed
    Then the current snapshot names "http: curl is not installed"

  Scenario: A failing custom probe is a problem
    Given the host declares a custom probe with a timeout
    And the command fails or exceeds its timeout
    Then the current snapshot names "probe <name>: failed"
```

### Reported, not attributed

```gherkin
  Scenario: A failure that was already there is reported, not attributed
    Given the baseline names the unit old.service
    And the current snapshot names old.service
    Then the verdict is DegradedPreExisting naming old.service
    And the verdict is not a failure
    And the host does not count as converged

  Scenario: A pre-existing loop that recovers and returns is a regression
    Given the baseline has no problem
    And the current snapshot names a unit in auto-restart
    Then the verdict is Degraded, not DegradedPreExisting
```

### Not observed

```gherkin
  Scenario: Disabled checks are unverified
    Given health checks are disabled for the host
    When the deployment finishes
    Then the verdict is Unverified with the reason that checks are disabled
    And the host does not count as converged

  Scenario: An unobservable host is unverified
    Given the probe cannot run, for example because SSH fails or times out
    Then the verdict is Unverified with the probe's reason
    And the host does not count as converged

  Scenario: A reboot-required deployment is pending, not healthy
    Given the activation wrote /run/reboot-required
    Then the verdict is PendingReboot
    And the host does not count as converged
```

---

## Feature: The fleet's verified marker

```gherkin
@health @verified @fleet
Feature: The verified marker is a complete observation, never an assumption

  Scenario: Every selected host must be reachable and healthy
    Given a revision was applied
    And every selected host is reachable
    And every selected host's last verdict is Healthy
    And every verdict is at most 1800 seconds old
    Then the revision is marked verified

  Scenario: One unhealthy host blocks the marker
    Given a revision was applied
    And one selected host is Degraded
    Then the revision is not marked verified

  Scenario: A missing baseline never marks verified
    Given no revision was applied
    Then the revision is not marked verified

  Scenario: A host gone beyond the threshold is outside the claim
    Given a selected host has been unreachable longer than the configured gone-after window
    And every other selected host is reachable, healthy and fresh
    Then the revision is marked verified
    And the gone host is still reported as gone

  Scenario: An outage inside the threshold blocks the marker
    Given a selected host has been unreachable for less than the gone-after window
    Then the revision is not marked verified

  Scenario: A stale verdict is refreshed before the marker is claimed
    Given a host's verdict is missing or at least 900 seconds old
    And the host is not known to be away
    Then the fleet re-observes that host without deploying
    And the refreshed verdict replaces the stored one

  Scenario: An away host is left to the reachability probe
    Given a host is known to be offline or misconfigured
    Then the health refresh does not probe it
```

---

## The verdict table

| Verdict | Means | Failure | Converged |
|---|---|---|---|
| `Healthy` | every enabled check passed; nothing was failing before | no | yes |
| `Degraded` | at least one problem appeared that the baseline did not have | yes | no |
| `DegradedPreExisting` | every problem was already there before the activation | no | no |
| `PendingReboot` | the new generation needs a reboot to take effect | no | no |
| `Unverified` | no observation was possible | no | no |

## Constants

| Name | Value | Meaning |
|---|---|---|
| `HEALTH_FRESHNESS_SECS` | 1800 | a verdict older than this cannot support the verified marker |
| `HEALTH_REFRESH_SECS` | 900 | a verdict older than this is re-observed between deployments |
| `healthChecks.timeoutSecs` | 30 | probe timeout for declared units, ports and probes |
| `healthChecks.systemd.maxRestarts` | 0 | allowed `NRestarts` increase per declared unit |
