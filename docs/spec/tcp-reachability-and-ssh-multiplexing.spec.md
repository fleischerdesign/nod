# Specification: TCP Socket Reachability & SSH Multiplexing

## Purpose
Ensure reliable reachability probes across environments blocking ICMP ping and accelerate multi-step SSH operations through connection multiplexing.

## Requirements & Acceptance Criteria

### 1. TCP Socket Reachability Probe
- **AC1**: `SshCliDeployer::check_reachability` MUST probe reachability using asynchronous TCP connect on `(host.target_host, host.target_port)` with a timeout bounded by `connect_timeout_secs` (default 3s).
- **AC2**: On successful TCP connection, `check_reachability` returns `Ok(true)`. On connection refusal, timeout, or DNS failure, it returns `Err(NodError::unreachable(host.name))`.
- **AC3**: Local deployment (`LocalDeployer::check_reachability`) remains local and returns `Ok(true)` without network probes.

### 2. SSH Connection Multiplexing
- **AC4**: `build_ssh_opts` MUST emit standard OpenSSH multiplexing options:
  - `-o ControlMaster=auto`
  - `-o ControlPath=/tmp/nod-ssh-%r@%h:%p`
  - `-o ControlPersist=60s`
- **AC5**: Multiplexing options MUST flow to `NIX_SSHOPTS` for store transfers as well as direct command executions.
- **AC6**: User-provided `extra_ssh_args` are appended afterwards, allowing specific overrides if needed.
