# git-vdb

[![CI](https://github.com/nishu-builder/git-vdb/actions/workflows/ci.yml/badge.svg)](https://github.com/nishu-builder/git-vdb/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

**A vector database stored in a Git repository.**

A Git repository holds the database state. When you use the CLI or the
`Database` / `Collection` API:

- each collection is named by `refs/git-vdb/collections/<name>`;
- creating a collection writes its initial tree and commit;
- every successful upsert or delete produces an immutable root tree, creates a
  new commit parented to the previous collection commit, and atomically advances
  the collection ref;
- queries read the root tree at the current ref, or at a historical root or
  commit you specify;
- collection history is ordinary Git commit history.

There is no database server or hidden state outside the repository. Git objects
hold the vectors, payloads, metadata, and search index. Git refs select the
current version of each named collection.

The root tree and commit have different roles. The tree is the database value:
the same configuration and point set always produce the same root object ID,
independent of insertion order, repository path, history, timestamps, or
machine. The commit records when and how that value became the current named
collection, so its identity may vary.

A no-op mutation can reproduce the same root tree ID, but the named-collection
layer still records the successful operation as a new commit.

The lower-level `SnapshotEngine` exposes the tree layer directly. Its `build`,
`apply`, and `query` operations use no collection name, commit, ref, clock, or
history. This is useful when another system already owns naming, caching, or
persistence and wants to pass the deterministic root around as a value.

```text
deterministic snapshot engine
    config + points  -> root tree
    root + mutations -> new root tree
    root + query     -> scored points

default CLI and named-collection adapter
    root tree -> commit -> refs/git-vdb/collections/<name>
```

The project’s central distinction is: **the tree is the database state; the
commit records history about that state; the ref names the current state.**

## Semantics

| Operation | Inputs | Result | Mutable side effects |
|---|---|---|---|
| `build` | configuration, points | deterministic root | writes immutable Git objects only |
| `apply` | previous root, mutations | deterministic new root | writes immutable Git objects only |
| `query` | root, vector, filter | scored points | none |
| named collection write | collection name, mutations | root and commit | atomically advances one collection ref |

### Named command semantics

- `collection create` creates an empty deterministic root, commits it, and
  creates the collection ref. It fails if that name already exists. `collection
  list` and `collection info` read collection refs and root metadata.
- `upsert` is both add and replace. A new typed ID is inserted; an existing
  typed ID is replaced by the submitted vector and payload. The whole batch is
  validated before the collection ref advances. Empty batches and duplicate IDs
  within one batch are rejected.
- `get` retrieves points without similarity scoring. IDs and a filter combine as
  an intersection. Results use canonical typed-ID order, then `offset` and
  `limit`; vectors and payloads are returned only when requested.
- `query` returns at most `limit` filter-eligible points ranked by descending
  cosine similarity, with canonical typed-ID order breaking equal-score ties.
  `--exact` scores every eligible point. Approximate mode uses deterministic LSH
  candidate discovery under explicit probe and candidate limits, so it may not
  return the true global top-k. Every result reports the resolved root and work
  statistics.
- `count` returns the number of points at the resolved root, optionally matching
  a filter. It performs no ranking.
- `delete` removes the union of the submitted IDs and filter matches. Missing IDs
  are ignored. A selector with neither IDs nor a filter is rejected. A successful
  no-op delete still creates a named-collection commit whose tree may equal its
  parent’s tree.
- `--expect-root` on `upsert` and `delete` rejects a stale writer before the ref
  update. The ref is also updated with an atomic compare-and-swap, so concurrent
  writers cannot silently overwrite one another.
- `--at <root-or-commit>` makes `get`, `count`, `query`, and `validate` read a
  historical immutable state. Historical collection views are read-only.
- `history` walks collection commits newest-first. `diff` compares two roots or
  commits and reports added, removed, and changed IDs plus structural reuse.
  `validate --full` additionally recomputes and checks every LSH bucket entry.
- `collection delete` deletes only the named ref. Its commits and trees remain
  subject to normal Git reachability and garbage collection.

Point reads, counts, queries, and validations identify the root they actually
read. Validation or write errors leave the collection ref unchanged; Git objects
produced before a failed ref update can remain unreachable until normal garbage
collection.

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
nix build
VDB=./result/bin/git-vdb

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

## Data model

A point has a typed string or unsigned-integer ID, one dense `f32` vector, and a
JSON object payload. String `"42"` and integer `42` are distinct IDs.

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

The flake is the canonical build and development interface:

```sh
nix run github:nishu-builder/git-vdb -- --help
nix develop
nix flake check
```

`flake.lock` pins Nixpkgs, Crane, the Rust overlay, and the compiler toolchain.
`nix build` produces the CLI at `./result/bin/git-vdb`; `nix run` executes it;
`nix develop` provides Cargo, rustfmt, Clippy, rust-analyzer, and Git; and
`nix flake check` runs the package build, formatting, Clippy,
tests, and rustdoc checks.

Cargo remains usable inside the development shell and on Windows. A direct
Cargo installation is also available:

```sh
cargo install --git https://github.com/nishu-builder/git-vdb.git --locked
```

Pin the flake input, commit, or release tag for reproducible production use.
Prebuilt binaries and a crates.io package are not published yet.

Development setup, invariants, and validation commands are in
[CONTRIBUTING.md](CONTRIBUTING.md). Report vulnerabilities privately as
described in [SECURITY.md](SECURITY.md).

Licensed under the [Apache License 2.0](LICENSE).
