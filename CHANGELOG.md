# Changelog

All notable changes to `git-vdb` are documented here. The project follows
[Semantic Versioning](https://semver.org/). Crate API compatibility and persisted
format compatibility are separate promises: a crate release may add APIs while
continuing to read and write the same canonical format version.

## [Unreleased]

## [0.4.0] - 2026-07-23

### Added

- added typed and batched document queries, facade-level detailed vector
  operations, and atomic mixed-mutation batches;
- added existence, inclusion/exclusion, array-containment, and stored-document
  substring/regex filters with validation;
- added local file indexing and text search to FastEmbed-enabled CLIs, bounded
  streaming JSONL imports, inline filters, pretty/JSONL output, diagnostics, and
  shell completions;
- added history-preserving restore plus fast-forward-only collection push/pull
  commands, Chroma migration guidance, and framework integration guidance.
- added a dependency-free Python client with Chroma-shaped vector CRUD, query,
  metadata/document filters, collection management, and diagnostics.

### Changed

- made document search the README front door, documented prebuilt release
  binaries, and made visible quickstart examples copy-pasteable;
- changed the implicit CLI database from the surrounding directory to the safer
  `.git-vdb` child directory;
- enabled the FastEmbed feature in release binaries so document indexing and
  text search work without rebuilding the CLI.

## [0.3.0] - 2026-07-23

### Added

- added a small persistent `Store` facade that safely opens or creates a local
  database, infers collection dimensions on first upsert, and exposes direct
  search, retrieval, deletion, count, and peek operations;
- added a provider-independent text collection that embeds documents and text
  queries through a user-supplied `Embedder` while persisting and checking the
  embedding-model identity;
- added an optional `fastembed` feature with a first-party local model adapter,
  cached offline inference, and a compiled end-to-end example;
- added executable quickstart, persistence, filtering, history, and text examples
  plus task-oriented guides and an agent-facing documentation map.

### Changed

- made the persistent open/collection/upsert/search workflow the README and
  crate-rustdoc front door while retaining the detailed database and immutable
  snapshot APIs;
- simplified CLI first use with safe database auto-creation, inferred collection
  dimensions, positional JSONL or stdin, inline points and vectors, and the
  primary `--db` and `search` spellings; retained `--repo`, `--input`, and
  `query` compatibility aliases;
- pinned the supported toolchain and declared Rust version to Rust 1.97,
  clearing the ONNX dependency blocker for the optional local text path.

## [0.2.0] - 2026-07-23

### Changed

- made deterministic sharded IVF-flat format version 2 the sole default for new
  roots, substantially reducing build, approximate-query, mutation, and storage
  costs while improving recall and filtered result completeness;
- retained read, validation, and mutation compatibility for canonical
  format-version-1 roots;
- replaced v1-only LSH creation flags with format-neutral query defaults in the
  CLI, exposed each root's persisted format in collection and snapshot info,
  and published the normative format-version-2 specification in rustdoc;
- reused the canonical codebook and unchanged assignments for sample-stable
  replacement batches, cutting the measured 100,000-point 1% upsert p50 by
  59.9-64.2% across arm64 and x86_64 without changing its resulting root;
- replaced per-candidate tree lookups with a compact shard-row search view,
  cutting the paired 100,000-point warm approximate-query p50 by 17.8-45.1%
  across x86_64 and arm64 with identical ordered results, scores, and work
  statistics.

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

[Unreleased]: https://github.com/nishu-builder/git-vdb/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/nishu-builder/git-vdb/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/nishu-builder/git-vdb/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/nishu-builder/git-vdb/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/nishu-builder/git-vdb/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/nishu-builder/git-vdb/releases/tag/v0.1.0
