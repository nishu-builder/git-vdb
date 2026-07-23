# Changelog

All notable changes to `git-vdb` are documented here. The project follows
[Semantic Versioning](https://semver.org/). Crate API compatibility and persisted
format compatibility are separate promises: a crate release may add APIs while
continuing to read and write the same canonical format version.

## [Unreleased]

### Changed

- made deterministic sharded IVF-flat format version 2 the sole default for new
  roots, substantially reducing build, approximate-query, mutation, and storage
  costs while improving recall and filtered result completeness;
- retained read, validation, and mutation compatibility for canonical
  format-version-1 roots;
- replaced v1-only LSH creation flags with format-neutral query defaults in the
  CLI, exposed each root's persisted format in collection and snapshot info,
  and published the normative format-version-2 specification in rustdoc.

## [0.1.1] - 2026-07-22

### Changed

- streamlined the README around installation, quick starts, the storage model,
  and links to the complete rustdoc and specifications;
- updated the benchmark-only PyArrow dependency from 21.0.0 to 23.0.1.

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

[Unreleased]: https://github.com/nishu-builder/git-vdb/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/nishu-builder/git-vdb/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/nishu-builder/git-vdb/releases/tag/v0.1.0
