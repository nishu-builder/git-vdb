# Changelog

All notable user-visible changes to this project will be documented here. The
format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
the project intends to follow [Semantic Versioning](https://semver.org/) once
public releases begin.

## Unreleased

### Added

- Ref-free `SnapshotEngine` build/apply/query APIs and immutable `Snapshot`
  handles.
- Exact-root export to and import from materialized directories.
- Open-source contribution, conduct, and security policies.
- Cross-platform CI, dependency review, supply-chain policy, and Dependabot.
- GitHub issue and pull-request templates.

### Changed

- Separated deterministic snapshot operations from the named Git commit/ref
  adapter; collection writes now use the snapshot engine.
- Upgraded `git2` to 0.21 to address RUSTSEC-2026-0183 and
  RUSTSEC-2026-0184.

## 0.1.0 - 2026-07-21

### Added

- Embedded Rust library and JSON CLI for Git-backed vector collections.
- Immutable deterministic roots, history, compare-and-swap writes, validation,
  and structural diff.
- Typed point IDs, JSON payloads, filters, exact cosine search, and deterministic
  multi-table random-hyperplane LSH.
- Canonical format documentation and deterministic benchmark harness.
