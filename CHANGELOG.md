# Changelog

All notable changes to `git-vdb` are documented here. The project follows
[Semantic Versioning](https://semver.org/). Crate API compatibility and persisted
format compatibility are separate promises: a crate release may add APIs while
continuing to read and write the same canonical format version.

## [Unreleased]

## [0.1.0] - 2026-07-22

Initial public release.

### Added

- deterministic immutable snapshot roots stored as canonical Git trees;
- named collections backed by commits and compare-and-swap refs;
- exact cosine search and deterministic LSH approximate search;
- typed IDs, JSON payload filters, point retrieval, counts, and deletion;
- historical reads, history, diffs, validation, materialization, and import;
- ref-free `SnapshotEngine` and named `Database` / `Collection` APIs;
- JSON CLI, formal rustdoc API documentation, and format-version-1 specification.

[Unreleased]: https://github.com/nishu-builder/git-vdb/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/nishu-builder/git-vdb/releases/tag/v0.1.0
