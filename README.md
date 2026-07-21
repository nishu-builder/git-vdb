# git-vdb

[![CI](https://github.com/nishu-builder/git-vdb/actions/workflows/ci.yml/badge.svg)](https://github.com/nishu-builder/git-vdb/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

**A vector database whose state is an immutable, deterministic Git tree.**

`git-vdb` treats a vector database snapshot as a value, not as a location or a
running service. A collection configuration and set of points produce one
canonical root tree. The same logical state produces the same root object ID,
independent of insertion order, repository path, commit history, timestamps, or
machine.

Git commits and refs are deliberately outside that identity. They are an
optional convenience layer for naming snapshots, recording history, coordinating
writers, and using normal fetch/push workflows.

```text
deterministic snapshot engine
    config + points  -> root tree
    root + mutations -> new root tree
    root + query     -> scored points

optional named-collection adapter
    root tree -> commit -> refs/git-vdb/collections/<name>
```

This distinction is the project’s central design choice: **the tree is the
database; the commit is history about the database.**

## Semantics

| Operation | Inputs | Result | Mutable side effects |
|---|---|---|---|
| `build` | configuration, points | deterministic root | writes immutable Git objects only |
| `apply` | previous root, mutations | deterministic new root | writes immutable Git objects only |
| `query` | root, vector, filter | scored points | none |
| named collection write | collection name, mutations | root and commit | atomically advances one collection ref |

The snapshot operations do not require a collection name, commit, ref, clock,
or repository history. Previous roots remain valid after `apply`. Reads never
change objects or refs.

Roots can be stored in a bare or non-bare Git object database, or materialized
as ordinary files and reopened without a surrounding repository. This makes a
root suitable as a cache key, worker result, reproducible build artifact, or
portable application value.

## Which layer should I use?

Use `SnapshotEngine` when the caller already owns naming, caching, scheduling,
or persistence. It exposes the ref-free `build` / `apply` / `query` boundary and
can use an ephemeral object database.

Use `Database` and `Collection`, or the CLI, when you want familiar mutable
collection names. This adapter adds parented commits, history, optimistic
expected-root checks, atomic ref updates, and Git transport. It delegates tree
construction to the same snapshot engine, so equivalent states have identical
roots in both layers.

## Quick start

Build the CLI and create a three-dimensional collection:

```sh
cargo build --release
VDB=./target/release/git-vdb

$VDB init ./demo-vectors.git --bare
$VDB --repo ./demo-vectors.git collection create products --dimension 3
```

Insert some points as JSON Lines:

```sh
cat > points.jsonl <<'EOF'
{"id":"red","vector":[1.0,0.0,0.0],"payload":{"label":"Red product"}}
{"id":"orange","vector":[0.9,0.1,0.0],"payload":{"label":"Orange product"}}
{"id":"yellow","vector":[0.7,0.3,0.0],"payload":{"label":"Yellow product"}}
{"id":"green","vector":[0.0,1.0,0.0],"payload":{"label":"Green product"}}
EOF

$VDB --repo ./demo-vectors.git upsert products --input points.jsonl
```

Return the three nearest points by cosine similarity:

```sh
printf '%s\n' '[1.0,0.0,0.0]' > query.json

$VDB --repo ./demo-vectors.git query products \
  --vector query.json --limit 3 --exact --with-payload
```

stdout is one compact JSON value; diagnostics go to stderr.

## Immutable Rust API

```rust
use git_vdb::{CollectionConfig, Point, Query, Snapshot, SnapshotEngine,
              SnapshotMutation};

# fn main() -> git_vdb::Result<()> {
let engine = SnapshotEngine::ephemeral()?;
let first = engine.build(
    CollectionConfig {
        dimension: 2,
        ..CollectionConfig::default()
    },
    vec![Point {
        id: "one".into(),
        vector: vec![1.0, 0.0],
        payload: Default::default(),
    }],
)?;

let next = engine.apply(
    first.root(),
    vec![SnapshotMutation::upsert(Point {
        id: "two".into(),
        vector: vec![0.8, 0.2],
        payload: Default::default(),
    })],
)?;

let result = engine.query(
    next.root(),
    Query {
        vector: vec![1.0, 0.0],
        limit: 2,
        ..Query::default()
    },
)?;

next.materialize("./snapshot")?;
let reopened = Snapshot::open_directory("./snapshot")?;
assert_eq!(reopened.root(), next.root());
assert_eq!(result.root, next.root());
# Ok(())
# }
```

`SnapshotEngine::open` uses an existing Git object database;
`SnapshotEngine::init` creates a bare one; `SnapshotEngine::ephemeral` manages a
temporary one for the lifetime of its snapshots. Exact-root APIs accept only a
full tree object ID. They intentionally do not resolve commits or symbolic refs.

## Data and query semantics

A point has a typed string or unsigned-integer ID, one dense `f32` vector, and a
JSON object payload. String `"42"` and integer `42` are distinct IDs.

Format version 1 uses cosine similarity. Exact search is the correctness oracle
and can be forced with `--exact`. Above the root’s configured full-scan
threshold, queries default to deterministic random-hyperplane LSH. Approximate
queries expose explicit probe and unique-candidate limits and report how much
work they performed.

Filters support scalar `match`, numeric `range`, `has_id`, nested groups, and
`must` / `should` / `must_not`. Dot-separated field paths traverse nested JSON
objects.

The current mutation implementation reconstructs the logical root from the
authoritative point set. Git reuses identical blobs and subtrees, but write CPU
cost is not yet proportional to the number of changed points.

## Canonical format and portability

The root contains canonical metadata, authoritative point trees, and a
deterministic LSH index:

```text
meta.json
points/<id-hash-prefix>/<id-hash>/
index/lsh-v1/<table>/<signature>/<id-hash>
```

JSON bytes, typed IDs, vector bytes, tree modes, projection generation, bucket
paths, probe order, scoring, and tie ordering are versioned. See
[docs/format.md](docs/format.md) for the normative format and
[docs/snapshots.md](docs/snapshots.md) for snapshot lifecycle and directory
materialization.

Materializing a root writes its exact tree as ordinary files with no `.git`
directory. Reimporting those files computes the same root ID. A filesystem
export expands Git’s structural sharing, so use an object database when compact
deduplication matters.

Ref-free roots are not automatically reachable from Git refs. If you keep them
in a long-lived object database, retain the root IDs externally and ensure Git
garbage collection does not prune them. The named-collection adapter handles
reachability by committing each root.

## Scope

`git-vdb` is embedded and offline. It has no server, daemon, network calls,
embedding model, authentication layer, or telemetry. Git transport is available
through ordinary Git tooling when the named-collection adapter is used.

Version 1 currently has one dense vector per point, cosine similarity only, no
payload index, and no sparse or hybrid search. The deterministic LSH defaults
are a starting point rather than a claim of production tuning; current benchmark
findings are recorded in [docs/findings.md](docs/findings.md).

## Install and contribute

Install the latest revision from source:

```sh
cargo install --git https://github.com/nishu-builder/git-vdb.git --locked
```

Pin a commit or release tag for reproducible production use. Prebuilt binaries
and a crates.io package are not published yet.

Development setup, invariants, and validation commands are in
[CONTRIBUTING.md](CONTRIBUTING.md). Report vulnerabilities privately as
described in [SECURITY.md](SECURITY.md).

Licensed under the [Apache License 2.0](LICENSE).
