# Specification: Fleet Topology Graph & Inventory Exporters (`nod graph`, `nod export`)

## Status
Approved (ADR-020)

## Motivation
Provide operators with topology visualization (`nod graph`) and standard inventory exports (`nod export`) for Ansible and Prometheus.

## Acceptance Criteria

### AC1: Topology Graph Rendering (`nod graph`)
- `nod graph` must discover fleet hosts from the flake.
- `--format mermaid` must emit a valid Mermaid `graph TD` block showing hosts, tags, roles, and architectures.
- `--format dot` must emit a valid Graphviz `digraph` definition.
- `--format json` must emit structured JSON topology.

### AC2: Inventory Export (`nod export`)
- `nod export ansible` must generate a valid Ansible YAML inventory grouping hosts by role and tag.
- `nod export prometheus` must generate a valid Prometheus HTTP/File Service Discovery JSON format mapping `__address__` (port 9100) and labels.
- `nod export json` must emit raw fleet configuration metadata.

### AC3: Quality Gate
- 100% test pass on `cargo test`.
- Zero warnings under `cargo clippy --all-targets -- -D warnings`.
- Format verified by `cargo fmt --check`.
