# ADR-023: TCP Socket Reachability Probe & SSH Connection Multiplexing

## Context

Previously, `nod` used `ping -c 1 -W 2 <target_host>` in `SshCliDeployer::check_reachability` to verify if a remote host was online. In real-world cloud and enterprise environments (such as AWS VPCs with strict security groups, Hetzner Cloud Firewalls, corporate VPNs, and Tailscale overlays), ICMP Echo Requests are frequently filtered or disabled, causing false-negative reachability failures even when SSH on port 22 (or a custom configured port) is completely functional.

Furthermore, remote deployment workflows (`switch`, `boot`, `test`, `reboot`, `drift`, `health`) invoke multiple sequential SSH commands per target host (`check_reachability`, `current_closure`, `nix copy` via `NIX_SSHOPTS`, `switch-to-configuration`, `health_check`). Establishing a full TCP 3-way handshake and SSH cryptographic key exchange for each separate command introduces significant connection latency across multi-host fleets.

## Decision

1. **TCP Socket Reachability Probe**:
   - Replace ICMP `ping` in `SshCliDeployer::check_reachability` with an asynchronous TCP connection probe using `tokio::net::TcpStream::connect((target_host, port))`.
   - The probe targets the resolved SSH port (`host.nod_config.ssh.port` or `host.target_port`, defaulting to 22) with a configurable connection timeout (`host.nod_config.ssh.connect_timeout_secs`, defaulting to 3s).
   - This eliminates false negatives caused by ICMP filtering while directly validating transport-layer socket reachability.

2. **SSH Connection Multiplexing (`ControlMaster`)**:
   - Configure OpenSSH multiplexing parameters in `src/domain/ssh_args.rs`:
     - `-o ControlMaster=auto`
     - `-o ControlPath=/tmp/nod-ssh-%r@%h:%p`
     - `-o ControlPersist=60s`
   - These options are included in `build_ssh_opts` and forwarded both to standalone `ssh` commands and to `nix copy --to ssh://` via `NIX_SSHOPTS`.
   - The initial SSH command establishes the master connection; subsequent operations on the same target host reuse the existing multiplexed Unix socket, reducing round-trip overhead.

## Consequences

- **Reliability**: Reachability checks succeed reliably across firewalled, cloud, and overlay networks where ICMP is blocked.
- **Performance**: Multi-step SSH operations against remote hosts execute with reduced connection overhead.
- **Compatibility**: Standard OpenSSH behavior is preserved without requiring host-side configuration changes.
