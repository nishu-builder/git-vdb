# Immutable snapshots

`git-vdb` has two layers:

```text
deterministic snapshot engine
  build(config, points) -> root
  apply(previous root, mutations) -> root
  query(root, vector, filter) -> scored points

named collection adapter
  commits + refs/git-vdb/collections/* + history + compare-and-swap
```

The snapshot engine is the portable data boundary. `SnapshotEngine::build`,
`apply`, and root-scoped reads operate on Git objects but never create a commit,
update a ref, resolve a symbolic revision, inspect history, or read the clock.
They accept and return deterministic tree IDs.

`SnapshotEngine::open` uses an existing bare or non-bare Git object database.
`SnapshotEngine::init` creates a bare object database intended for caller-managed
roots. `SnapshotEngine::ephemeral` supplies an isolated, automatically retained
temporary object database when the caller only needs a snapshot value or plans
to materialize it. `open_snapshot` accepts only a full tree object ID; commit IDs
and ref names are intentionally rejected.

## Directory snapshots

`Snapshot::materialize` writes a root as ordinary files and directories with no
`.git` directory. `Snapshot::open_directory` imports those files into an
isolated temporary object database and recomputes the root. The returned root is
identical because the directory is the exact canonical Git tree representation.
The source directory is not modified and remains independent of subsequent
`apply` calls.

`SnapshotEngine::build_directory` is a convenience for building and
materializing without supplying an object database. To retain imported objects
in a caller-owned database, use `SnapshotEngine::import_directory`; the
repository-free `Snapshot::open_directory` variant retains an isolated temporary
database for the lifetime of its returned handle.

Materialization expands shared Git objects at every tree path. In particular,
index entries reference the same point trees inside Git but become repeated file
content in a directory. Use a Git object database when compact structural
sharing matters.

## Retention

Ref-free roots are deliberately not made reachable by Git refs. A caller using a
long-lived object database must retain root IDs externally and ensure repository
garbage collection does not prune the corresponding objects. Materialized
directories retain their own contents. The named collection adapter provides
Git reachability by committing each root and advancing its collection ref.

## Named collections

`Database` and `Collection` are the convenience layer for interactive and CLI
use. They preserve the existing collection names, parented commit history,
historical reads, optimistic expected-root checks, fetch/push compatibility, and
atomic ref updates. Their deterministic tree construction is delegated to the
same snapshot engine, so rebuilding an equivalent point set through either layer
produces the same root.

## Selective object reads

`SnapshotReader<S>` queries a canonical format-2 root through a
`SnapshotSource`. `GitSource` reads directly from a pinned local Git tree;
`DirectorySource` reads a materialized directory. External stores can implement
`read_blob(path)` and `list(path)` to fetch only visited objects.

```rust,no_run
use git_vdb::{GitSource, ObjectId, Query, SnapshotReader};
# fn main() -> git_vdb::Result<()> {
let root: ObjectId = "0123456789012345678901234567890123456789".parse()?;
let source = GitSource::new("vectors.git", &root)?;
let mut reader = SnapshotReader::open(root, source)?;
let hits = reader.query(Query::approximate([1.0, 0.0], 10).with_payload())?;
println!("{:?}", reader.read_stats());
# Ok(()) }
```

Approximate queries load metadata, the codebook, selected postings, and
candidate ID/vector shards. Unfiltered searches load payload shards only for
winners. Filters can require payloads before scoring. Exact queries read all
ID/vector shards. Objects decoded within a query are reused, then dropped;
the source owns caching between queries.

`read_stats()` counts requested blobs, their uncompressed byte lengths, and
directory listings, including metadata read at opening. It does **not** count
compressed network traffic, tree bytes, physical disk reads, or cache misses.
Format 2 still transfers whole shards: small candidate counts may touch most
of its 64 shards. Existing `Snapshot` handles retain their decoded caches and
are often better for repeated queries in a long-lived process.

The source must serve immutable objects bound to the supplied root.
`DirectorySource` rejects accessed symlinks but does not authenticate that
directory's Git identity. The reader validates visited objects and metadata;
it does not promise validation of unvisited payloads, postings, or the training
sample. Use `Snapshot::open_directory` and `Snapshot::validate(true)` to import
and fully validate an untrusted directory. Existing snapshot APIs continue to
read and mutate formats 1 and 2; this additional reader supports format 2 only.
