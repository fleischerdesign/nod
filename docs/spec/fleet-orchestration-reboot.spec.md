# Specification: Fleet Orchestration Reboot (`nod reboot`)

## Status
Approved (ADR-017)

## Motivation
Provide operators with safe, automated, and verifiable host reboot workflows across single machines or entire clusters.

## Acceptance Criteria

### AC1: Multi-Host Reboot Orchestration
- `nod reboot` must resolve target hosts matching `[TARGET]`, `--tag`, `--role`, or `--all`.
- It must support rollout strategies (`batch`, `canary`, `all`) with configurable wave sizing (`--batch-size N`) and concurrency limits (`--concurrency N`).

### AC2: Online Recovery Verification
- By default, `nod reboot` must wait for rebooted hosts to cycle offline and verify their recovery online (SSH reachability + systemd health).
- `--no-wait` must trigger the reboot command asynchronously without polling.
- `--timeout <SECS>` must set a maximum recovery wait deadline (default: 180s).

### AC3: Quality Gate & Audit Log
- Successful and failed reboots must be appended to the deployment audit history.
- 100% test pass on `cargo test`.
- Zero warnings under `cargo clippy --all-targets -- -D warnings`.
- Code format verified by `cargo fmt --check`.
