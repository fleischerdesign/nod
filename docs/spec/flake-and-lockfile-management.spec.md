# Specification: Flake & Lockfile Management

## Status
Approved (ADR-014)

## Motivation
Provide operators with native commands to inspect flake inputs, metadata, and execute safe, observable input updates with change summaries.

## Acceptance Criteria

### AC1: Flake Inputs Inspection (`nod inputs`)
- `nod inputs` must inspect the target flake's lockfile and report all input nodes.
- For each input node, it must display the input name, original source (e.g. `github:NixOS/nixpkgs/nixpkgs-unstable`), locked revision (short 7-char hash), and last modified UTC date.
- With `--json`, it must output the structured list in JSON format.

### AC2: Flake Metadata Inspection (`nod metadata`)
- `nod metadata` must inspect the flake and display root details:
  - Flake path and URL
  - Current git revision and revision count
  - Lockfile format version
  - Total input nodes count
  - Last modified UTC date
- With `--json`, it must output the metadata as JSON.

### AC3: Flake Input Update (`nod update`)
- `nod update` with no arguments must update all inputs via `nix flake update`.
- `nod update <input1> <input2>...` must update only the specified inputs via `nix flake update <input1> <input2>...`.
- Upon completion, `nod update` must compute and render the diff between previous and updated revisions:
  - If inputs were updated, print: `<input>: <old_rev> -> <new_rev> (<old_date> -> <new_date>)`.
  - If no inputs changed, print: `✓ All inputs are already up to date.`
- If `nix flake update` fails, a strongly-typed `NodError::Evaluation` error must be returned.

### AC4: Quality Gate
- 100% test pass on `cargo test`.
- Zero warnings under `cargo clippy --all-targets -- -D warnings`.
- Code format verified by `cargo fmt --check`.
