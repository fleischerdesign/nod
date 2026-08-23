# nod — SSH Command Specification

> Gherkin feature specification for `nod ssh` (ADR-006 single-target
> invariant, `001-clean-architecture.md` split of presentation vs
> application). SSH argument construction lives in `src/commands/ssh.rs`
> (`build_ssh_args`, a pure function), host resolution is delegated to
> `TargetSelection::select_exact_one`, and the connection itself is a plain
> `ssh` child process with inherited stdio — no Nix or SSH evaluation on the
> command path beyond host discovery.

---

## Fixture: Fleet under test

```gherkin
@fixture @fleet
Feature: A stable fleet used by every ssh scenario

  Scenario: the fixture fleet
    Given the discovered fleet:
      | name          | tags           | role      |
      | web-01        | prod, web      | server    |
      | web-02        | prod, web      | server    |
      | db-01         | prod, db       | server    |
      | edge-prod     | prod, edge     | notebook  |
    Then the fleet has 4 hosts
```

---

## Feature: Single Host Resolution

```gherkin
@ssh @target
Feature: `nod ssh` resolves exactly one host before opening a session

  Scenario: an exact host name resolves one host
    Given the fixture fleet
    When `nod ssh web-01` resolves its target
    Then exactly one host (web-01) is selected
    And the SSH profile is derived via `SshProfile::for_host`

  Scenario: a glob matching several hosts is rejected
    Given the fixture fleet
    When `nod ssh "web-*"` resolves its target
    Then the command fails with a typed `NodError` naming `multiple hosts matched`
    And the error lists [web-01, web-02]

  Scenario: no matching hosts is rejected
    Given the fixture fleet
    When `nod ssh "nowhere-*"` resolves its target
    Then the command fails with a typed `NodError` naming `no hosts matched`

  Scenario: a tag selecting several hosts is rejected
    Given the fixture fleet
    When `nod ssh --tag prod` resolves its target
    Then the command fails with a `multiple hosts matched` error
```

---

## Feature: SSH Argument Construction

```gherkin
@ssh @build-args
Feature: `build_ssh_args` derives the ssh(1) argument vector from a host profile

  Scenario: default root user on the default port
    Given a profile with user `root` and port `22`
    When building arguments for host `atlas`
    Then the argument vector is exactly [root@atlas]

  Scenario: a custom port emits -p
    Given a profile with port `2200`
    When building arguments for a host
    Then the argument vector contains [-p, 2200, root@atlas]

  Scenario: an identity file emits -i
    Given a profile with identity file `/path/key`
    When building arguments for a host
    Then the argument vector contains [-i, /path/key, root@atlas]

  Scenario: a proxy jump emits -J
    Given a profile with proxy jump `bastion`
    When building arguments for a host
    Then the argument vector contains [-J, bastion, root@host]

  Scenario: extra ssh arguments are preserved positionally
    Given a profile with extra ssh args `-o KeepAlive=1`
    When building arguments for a host
    Then the argument vector includes the extra args before the target
```

---

## Feature: Interactive vs Remote Command Mode

```gherkin
@ssh @mode
Feature: `nod ssh` opens a shell or runs a remote command

  Scenario: no trailing command opens an interactive shell
    When `nod ssh web-01` runs without a trailing command
    Then the child process is a terminal session on the selected host
    And stdio is inherited from the invoking terminal

  Scenario: trailing arguments execute a remote command
    When the operator runs `nod ssh host -- uname -a`
    Then the child ssh receives the command bytes [uname, -a]

  Scenario: --sudo with a trailing command prepends sudo
    When the operator runs `nod ssh host --sudo -- apt-get update`
    Then the remote command bytes are [sudo, apt-get, update]

  Scenario: --sudo with no trailing command opens a root shell
    When `nod ssh host --sudo` runs without a trailing command
    Then the remote session requests an interactive root shell
    And the argument vector ends in [sudo, -i]
```

---

## Feature: Local Host Invocation

```gherkin
@ssh @local
Feature: a directly addressed local host may run its shell/command locally

  Scenario: a local host with a remote command
    Given a host whose `is_local` is true and no remote target is configured
    When `nod ssh local -- uname -a` runs
    Then the command runs on the local machine without ssh

  Scenario: a local host with no command opens the local shell
    Given a local host with no remote target configured
    When `nod ssh local` runs
    Then the operator is dropped into the local terminal shell
    And stdio is inherited from the running terminal

  Scenario: --sudo on a local host escalates the local shell/command
    Given a local host with no remote target configured
    When `nod ssh --sudo` runs without a trailing command
    Then the local session requests a root shell
```

---

## Feature: Execution Failure Surfaces a Typed Error

```gherkin
@ssh @error
Feature: a failed or failing child process is reported as a NodError

  Scenario: the ssh child process exits non-zero
    Given a host that resolves to one target
    When the `ssh` child process exits with a non-zero status
    Then `execute` returns a typed `NodError` deployment error

  Scenario: the ssh child cannot be launched
    When the `ssh` executable cannot be spawned
    Then `execute` returns a typed `NodError` deployment error
```

---

> **Conventions:** `nod ssh` is a single-terminal command. Target resolution
> always goes through `TargetSelection::select_exact_one` (ADR-006), so 0 or >1
> matches are clean, typed configuration errors — never a silent host pick. The
> ssh child process inherits `stdin`/`stdout`/`stderr` from the terminal so
> interactive sessions and long-running commands behave as native ssh(1).
> Argument construction (`build_ssh_args`) is a pure function over an
> `SshProfile` so it is unit-testable without any ssh/Nix/network.