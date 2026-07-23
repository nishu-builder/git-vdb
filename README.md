# git-vdb

[![CI](https://github.com/nishu-builder/git-vdb/actions/workflows/ci.yml/badge.svg)](https://github.com/nishu-builder/git-vdb/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/git-vdb.svg)](https://crates.io/crates/git-vdb)
[![docs.rs](https://img.shields.io/docsrs/git-vdb/latest?label=docs.rs)](https://docs.rs/git-vdb/latest/git_vdb/)
[![MSRV](https://img.shields.io/badge/MSRV-1.87-blue.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

A local vector database stored entirely in Git.

`git-vdb` is an embedded Rust library and CLI: no server, daemon, account, or
external database. Open a directory, write vectors, and search.

## Install

```sh
cargo add git-vdb       # Rust library
cargo install git-vdb   # CLI
```

## Rust quickstart

```rust
use git_vdb::{open, Point};

# fn main() -> git_vdb::Result<()> {
let db = open("./vectors.git")?;
let docs = db.collection("docs");
docs.upsert([
    Point::new("east", [1.0, 0.0]),
    Point::new("north", [0.0, 1.0]),
])?;
let hits = docs.search([0.9, 0.1], 1)?;
assert_eq!(hits[0].id.to_string(), "east");
# Ok(())
# }
```

The database and collection are created on the first write. Vector dimensions
are inferred, payloads are returned by default, and search automatically chooses
exact or approximate execution.

## CLI quickstart

```sh
git-vdb --db vectors.git upsert docs --id east --vector '1,0'
git-vdb --db vectors.git upsert docs --id north --vector '0,1'
git-vdb --db vectors.git search docs --vector '0.9,0.1' --limit 1
```

Batch upserts accept a JSON Lines file or `-` for stdin.

## Why Git-native?

- **Local:** one directory and no service to operate.
- **Versioned:** commits and refs provide optional history and atomic writes.
- **Deterministic:** equivalent data produces the same root tree.
- **Portable:** clone, fetch, push, archive, inspect, and maintain it with Git.

## Guides

- [Five-minute quickstart](docs/quickstart.md)
- [Persistence and reopening](docs/persistence.md)
- [Filtering and detailed queries](docs/filtering.md)
- [History and Git transport](docs/history.md)
- [Text and embedding models](docs/embeddings.md)
- [Rust API reference](https://docs.rs/git-vdb)

Advanced users can work directly with immutable snapshots, historical roots,
compare-and-swap writes, validation, and ANN controls through `Database`,
`Collection`, and `SnapshotEngine`.

## Internals and benchmarks

- [Canonical format version 2](docs/format-v2.md)
- [Legacy format version 1](docs/format.md)
- [Snapshot design](docs/snapshots.md)
- [Current limitations and findings](docs/findings.md)
- [LanceDB performance evidence](docs/benchmarks/lancedb-performance.md)

See [CONTRIBUTING.md](CONTRIBUTING.md) for development and validation. Licensed
under the [Apache License 2.0](LICENSE).
