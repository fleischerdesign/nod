# nod — Remote Command Execution Specification

> Gherkin feature specification for `nod exec` (ADR-006 unified target
> selection, ADR-005 fleet concurrency). Host resolution is delegated to
> `TargetSelection::select`; per-host execution lives in
> `ExecFleetUseCase` (`src/application/use_cases/exec_fleet.rs`) which
> derives each host's `SshProfile` and runs the command over `ssh` — or
> locally through `sh -c` when `host.is_local` is true. Presentation
> (`src/commands/exec.rs`) renders per-host prefixed output or a JSON result
> array, and surfaces an aggregate `NodError` when any host failed.

---

## Fixture: Fleet under test

```gherkin
@fixture @fleet
Feature: A stable fleet used by every exec scenario

  Scenario: the fixture fleet
    Given the discovered fleet:
      | name      | tags         | role      |
      | web-01    | prod, web    | server    |
      | web-02    | prod, web    | server    |
      | db-01     | prod, db     | server    |
      | edge-prod | prod, edge   | notebook  |
    Then the fleet has 4 hosts
```

---

## Feature: Multi-Host Target Resolution

```gherkin
@exec @target
Feature: `nod exec` resolves hosts with the ADR-006 selector grammar

  Scenario: an exact target selects one host
    Given the fixture fleet
    When `nod exec db-01 -- uptime` resolves its target
    Then exactly the host db-01 is executed against

  Scenario: a glob target selects every matching host
    Given the fixture fleet
    When `nod exec web-* -- uptime` resolves its target
    Then exactly the hosts [web-01, web-02] are executed against

  Scenario: --tag narrows the selected set
    Given the fixture fleet
    When `nod exec --tag db -- uptime` resolves its target
    Then exactly the host db-01 is executed against

  Scenario: --role narrows the selected set
    Given the fixture fleet
    When `nod exec --role notebook -- uptime` resolves its target
    Then exactly the host edge-prod is executed against

  Scenario: --all selects every discovered host
    Given the fixture fleet
    When `nod exec --all -- uptime` resolves its target
    Then all 4 discovered hosts are executed against

  Scenario: target, tag and role compose by boolean AND
    Given the fixture fleet
    When `nod exec "*-*" --tag prod --role server -- uptime` resolves its target
    Then only hosts carrying both the tag and the role are executed against

  Scenario: no criteria defaults to the local host
    Given a local host named `jello` in the fleet
    When `nod exec -- echo hi` resolves its target without any criteria
    Then exactly the local host is executed against

  Scenario: an unmatched target is a clean error
    Given the fixture fleet
    When `nod exec "nowhere-*" -- uptime` resolves its target
    Then the command fails with a typed `NodError` naming `no hosts matched`
```

---

## Feature: Concurrent Execution

```gherkin
@exec @concurrency
Feature: `nod exec` bounds in-flight hosts with a semaphore

  Scenario: --concurrency N bounds in-flight executions
    Given 6 hosts each running a 200ms command
    When `nod exec --concurrency 2 -- <command>` runs
    Then the wall-clock duration is approximately 3 command lengths
    And the in-flight host count never exceeds 2 at any moment

  Scenario: the default concurrency is 4
    Given a fleet of hosts
    When `nod exec` runs without `--concurrency`
    Then the semaphore is created with 4 permits

  Scenario: --concurrency 0 is rejected
    When `nod exec --concurrency 0 -- uptime` runs
    Then the command fails with a typed `NodError` mentioning `--concurrency`

  Scenario: an empty host set is rejected
    Given the selection resolves to no hosts
    When `nod exec` attempts to execute
    Then the command fails with a typed `NodError`
```

---

## Feature: Streaming Per-Host Output

```gherkin
@exec @format
Feature: `nod exec` streams each host's output with a host prefix

  Scenario: stdout lines carry the host prefix
    When `nod exec web-01 -- echo hello` runs on web-01
    Then every stdout line is emitted as `[web-01] hello`

  Scenario: empty stdout stays silent per line but the host still reports
    When `nod exec web-01 -- true` runs
    Then the host header `[web-01]` is echoed with the command
    And the exit status line reads `exit 0`

  Scenario: stderr lines share the same prefix
    Given a command that writes to stderr
    When `nod exec web-01 -- <stderr-writer>` runs
    Then every stderr line is emitted as `[web-01] <line>`

  Scenario: a summary counts succeeded, failed and skipped hosts
    When a run finishes with 2 successful, 1 failed and 1 skipped host
    Then a trailing summary line reports `2 succeeded, 1 failed, 1 skipped`
```

---

## Feature: Structured JSON Output

```gherkin
@exec @json
Feature: `--json` emits one structured object per host

  Scenario: every host result becomes a JSON object
    Given a run over [web-01, db-01]
    When `nod exec --json -- uptime` completes
    Then stdout is exactly a 2-element JSON array

  Scenario: each object carries the documented fields
    Given a host `web-01` that exited 0 in 12ms with stdout `up` and no stderr
    When the result is serialized
    Then the object is
      ```
      { "host": "web-01", "exit_code": 0, "stdout": "up", "stderr": "", "duration_ms": 12 }
      ```

  Scenario: a non-zero exit is captured verbatim
    Given a host whose command exited with code 3
    When the result is serialized
    Then the object's `exit_code` is exactly `3`
    And `stderr` carries the remote error text
```

---

## Feature: Sudo Escalation

```gherkin
@exec @sudo
Feature: `--sudo` prepends `sudo` to the remote command

  Scenario: remote hosts receive the sudo prefix
    When `nod exec db-01 --sudo -- apt-get update` builds the ssh argument vector
    Then the remote command bytes are [sudo, apt-get, update]

  Scenario: local hosts receive the sudo prefix inside sh -c
    Given a local host
    When `nod exec local --sudo -- echo root` builds the local invocation
    Then the local command is [sh, -c, sudo echo root]

  Scenario: without --sudo no escalation is added
    When `nod exec db-01 -- apt-get update` builds the ssh argument vector
    Then the remote command bytes are exactly [apt-get, update]
```

---

## Feature: Fail-Fast Abort

```gherkin
@exec @fail-fast
Feature: `--fail-fast` stops scheduling hosts after the first failure

  Scenario: a failing host aborts the remaining executions
    Given a fleet [bad, a, b, c] under `--concurrency 1`
    When `nod exec --fail-fast -- <failing-command>` runs
    Then `bad` reports its real non-zero exit code
    And the hosts [a, b, c] report exit code -1 and are marked skipped

  Scenario: without --fail-fast every host still runs
    Given a fleet [bad, good]
    When `nod exec` runs without `--fail-fast`
    Then `good` executes and succeeds even though `bad` failed
```

---

## Feature: Error Handling

```gherkin
@exec @error
Feature: failed hosts and unreachable transports are surfaced

  Scenario: any failed host fails the command
    When a run completes with at least one failed host
    Then `execute` returns a typed `NodError` deployment error
    And the summary is printed before the error is returned

  Scenario: a non-zero exit code marks the host failed
    Given a host whose command exits non-zero
    Then `ExecResult.success` is false for that host
    And `ExecResult.exit_code` equals the remote exit status

  Scenario: a transport that cannot be launched is a per-host failure
    Given an environment where the ssh transport cannot spawn
    When the host runs
    Then the host's `ExecResult` reports failure with a launch message
    And the run still returns per-host results instead of panicking
```

---

> **Conventions:** `nod exec` is a multi-host command; it resolves 0..n hosts
> through `TargetSelection::select` (ADR-006) and never guesses a host.
> Concurrency is enforced by a `tokio::sync::Semaphore` (ADR-005); all hosts
> are spawned immediately but only `N` run at once. Per-host results
> (`ExecResult`) carry `host_name`, the numeric `exit_code` (the ssh/sh child
> status code, or `-1` when `--fail-fast` skipped the host before it
> started), captured `stdout`/`stderr`, wall-clock `duration_ms` and a
> `success` flag. The command returns `Ok(())` when every host succeeded and
> a typed `NodError` otherwise, after the results have been rendered.