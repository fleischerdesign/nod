# Specification: Remote Closure Diff Preview

## Status
Approved (ADR-012)

## Motivation
`nod diff` must provide parity across all fleet hosts regardless of whether they are local machines or remote servers managed over SSH. Operators need to preview exact package and systemd unit changes before executing `nod switch`.

## Acceptance Criteria

### AC1: Polymorphic Active Closure Resolution
- `nod diff` must resolve the target host's active closure using `DeployerPort::current_closure(&host, &profile)`.
- The presentation layer must not branch on `host.is_local` for retrieving active closure paths.

### AC2: In-Sync Short-Circuiting
- If `active_closure == Some(new_closure)`, `nod diff` must output:
  `✓ System is already in sync with target closure (no package changes).`
- No diff subprocess (`nvd` or `nix store diff-closures`) shall be executed when closures are identical.

### AC3: Semantic Diff Tooling
- When `active_closure != Some(new_closure)` and an active closure exists, `nod diff` must execute `nvd diff <active_closure> <new_closure>`.
- If `nvd` is unavailable or exits non-zero, it must gracefully fall back to `nix store diff-closures <active_closure> <new_closure>`.
- If the diff tool outputs empty content (indicating no package changes despite divergent closure hashes), `nod diff` must explicitly report:
  `✓ No package version changes detected between closures.`

### AC4: Initial Deployment Notice
- If `active_closure` resolves to `None` (e.g. fresh bootstrap host), `nod diff` must informatively report that no previous generation exists.

### AC5: Quality Gate
- 100% test pass on `cargo test`.
- Zero warnings under `cargo clippy --all-targets -- -D warnings`.
- Code format verified by `cargo fmt --check`.
