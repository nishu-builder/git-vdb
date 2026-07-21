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
