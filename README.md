# git-vdb

[![CI](https://github.com/nishu-builder/git-vdb/actions/workflows/ci.yml/badge.svg)](https://github.com/nishu-builder/git-vdb/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/git-vdb.svg)](https://crates.io/crates/git-vdb)
[![docs.rs](https://img.shields.io/docsrs/git-vdb/latest?label=docs.rs)](https://docs.rs/git-vdb/latest/git_vdb/)
[![MSRV](https://img.shields.io/badge/MSRV-1.87-blue.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

A Git-native embedded vector database.

`git-vdb` stores vectors, payloads, metadata, and search indexes as immutable
Git objects. Equivalent data produces the same root tree, while ordinary Git
commits and refs provide optional naming, history, transport, and atomic writes.

## Install

```sh
cargo add git-vdb       # Rust library
cargo install git-vdb   # CLI
```

The project also provides a pinned Nix development and build environment:

```sh
nix run github:nishu-builder/git-vdb -- --help
```

## Rust quick start

```rust
use git_vdb::{CollectionConfig, Point, Query, SnapshotEngine};

# fn main() -> git_vdb::Result<()> {
let engine = SnapshotEngine::ephemeral()?;
let snapshot = engine.build(
    CollectionConfig::new(2),
    vec![
        Point::new("east", [1.0, 0.0]),
        Point::new("north", [0.0, 1.0]),
    ],
)?;

let result = snapshot.query(Query::exact([0.9, 0.1], 1))?;
assert_eq!(result.points[0].id.to_string(), "east");
# Ok(())
# }
```

See the [API documentation](https://docs.rs/git-vdb) for named collections,
immutable snapshots, filtering, approximate search, validation, history, and
materialization.

## CLI quick start

```sh
git-vdb init ./vectors.git --bare
git-vdb --repo ./vectors.git collection create products --dimension 3
git-vdb --repo ./vectors.git upsert products --input points.jsonl
git-vdb --repo ./vectors.git query products \
  --vector query.json --limit 10 --exact --with-payload
```

Inputs and outputs are JSON or JSON Lines. Run `git-vdb --help` or
`git-vdb <command> --help` for the complete command reference.

## Model

```text
tree   = deterministic database state
commit = history describing a state transition
ref    = mutable name for the current state
```

Use `SnapshotEngine` when another system owns naming and persistence. Use
`Database`, `Collection`, or the CLI when you want named collections backed by
commits and compare-and-swap refs. Both layers produce the same canonical roots.

Core capabilities include:

- typed string and unsigned-integer IDs, dense `f32` vectors, and JSON payloads;
- exact cosine search and deterministic LSH approximate search;
- Boolean payload filters, retrieval, counts, upserts, and deletion;
- immutable historical reads, diffs, validation, and portable snapshots;
- bare or non-bare Git repositories with no server or hidden external state.

Approximate search is explicitly bounded and may omit globally better points;
query results report the selected mode and work statistics.

## Documentation

- [Rust API documentation](https://docs.rs/git-vdb)
- [Canonical format version 1](docs/format.md)
- [Snapshot lifecycle and portability](docs/snapshots.md)
- [Current limitations and findings](docs/findings.md)
- [Contributing and validation](CONTRIBUTING.md)
- [Release process](RELEASING.md)
- [Security policy](SECURITY.md)

Persisted format version 1 and the crate's Semantic Version are separate
compatibility boundaries. The normative format specification defines canonical
bytes, paths, scoring, and ordering.

## Development

```sh
nix develop
nix flake check
```

Licensed under the [Apache License 2.0](LICENSE).
