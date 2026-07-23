# Simple Embedded API and Documentation

> **Status:** Implemented
>
> **Author:** Codex, for the project maintainer
>
> **Created:** 2026-07-23

## Summary

Make `git-vdb` feel as direct as a small embedded database: open one directory,
select a collection, upsert points, and search. The primary Rust and CLI paths
will hide Git object mechanics, collection configuration, and ANN tuning while
the existing snapshot and database APIs remain available as an advanced layer.
After the vector path is simple and stable, an optional first-party text layer
will provide document-in/text-query ergonomics without making a server or model
runtime part of the core crate.

The intended position is: **Chroma-like ergonomics, SQLite-like deployment,
and Git-native storage.**

## Problem

The storage engine is usable, documented, and correctness-tested, but its first
five minutes expose concepts that most users do not need:

- the README starts with `SnapshotEngine::ephemeral`, `CollectionConfig`, and
  an explicitly exact `Query` rather than a persistent collection;
- opening a durable database and creating a collection are separate operations;
- callers must know a vector dimension before the first write;
- the common search path constructs a detailed query object and unwraps a
  detailed result object;
- CLI users must initialize a repository, create a collection, specify its
  dimension, and put even small vectors in files;
- users with text must separately choose, run, and consistently identify an
  embedding model.

These are reasonable advanced controls, but they are currently the front door.
A user should not need to understand Git trees, roots, refs, compare-and-swap,
format versions, IVF partitions, probes, or candidate limits to persist and
search vectors.

## Solution

Add a deliberately small high-level facade and make it the documented default:

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

`open` opens an existing bare or non-bare repository and creates a bare
repository when the path does not exist. `collection` returns a lightweight
handle without performing I/O. The first nonempty upsert creates a missing
collection and infers its dimension; later writes validate that dimension.
`search` uses the existing automatic exact/approximate policy, includes payload
metadata by default, and returns the winning points directly. Detailed query
statistics and immutable roots remain available through `Collection::query` and
the existing advanced APIs.

The matching CLI path will be:

```sh
git-vdb --db vectors.git upsert docs points.jsonl
git-vdb --db vectors.git search docs --vector '0.9,0.1' --limit 5
```

Both commands open or initialize the database. A first upsert creates the
collection and infers its dimension. File input, stdin, and inline vectors are
supported; existing explicit initialization, collection creation, historical
reads, validation, and tuning controls remain available.

## Goals

- [x] A persistent vector search quickstart is no more than 15 meaningful Rust
      lines and mentions no Git or ANN implementation concepts.
- [x] `git_vdb::open(path)` safely opens or creates a local database.
- [x] The high-level collection handle creates a missing collection on its first
      upsert and deterministically infers vector dimension.
- [x] Concurrent first writes either converge through normal upsert semantics or
      return a clear dimension/configuration conflict; they never overwrite a
      collection ref silently.
- [x] `search(vector, limit)` returns winners directly, includes payloads, and
      preserves the existing automatic exact/approximate selection.
- [x] Common helpers cover ID retrieval, ID deletion, count, and peek without
      request-structure boilerplate.
- [x] The CLI can reach a first persisted query without explicit `init`,
      `collection create`, dimension flags, or temporary query-vector files.
- [x] README and rustdoc lead with the high-level persistent API; snapshots,
      roots, format documents, and index tuning are presented as advanced topics.
- [x] Quickstart, persistence, filtering, and history examples are executable in
      CI and use the same API shown in the documentation.
- [x] An optional first-party text layer supports document upsert and text search,
      persists an embedding-model identity, and keeps model dependencies out of
      the default core build.
- [x] Existing format-v2 roots remain canonical, format-v1 roots remain readable
      and mutable, and the high-level facade produces the same roots as the
      existing APIs for equivalent inputs.
- [x] The final crate archive, rustdoc, MSRV, Nix, CLI smoke tests, and all current
      correctness tests pass with no unrelated work included.

## Non-Goals

- A required HTTP server or background daemon.
- Clustering, sharding, replicas, tenants, authentication, or a control plane.
- User-managed index build jobs or multiple index families in the primary API.
- A schema language beyond vector dimension and JSON metadata.
- A dashboard or administrative web UI.
- Cloud hosting or synchronization beyond ordinary Git transport.
- Python, TypeScript, LangChain, or LlamaIndex bindings in this implementation
  goal. They are follow-up work after the core facade stabilizes.
- Removing `Database`, `Collection`, `SnapshotEngine`, detailed `Query`, history,
  validation, or compare-and-swap APIs.
- Changing the persisted format solely for API ergonomics.

## Design

### 1. Public API layers

The crate will have one obvious default layer and one explicit advanced layer:

- `open` and `Store` are the primary entry points.
- `Store::collection` returns a lazy `CollectionHandle`.
- `CollectionHandle` provides common operations with small signatures.
- Existing `Database`, `Collection`, `SnapshotEngine`, model types, and detailed
  results remain public and power advanced workflows.

The facade delegates to the existing engine rather than implementing a parallel
storage path. Public documentation will describe advanced types after the
quickstart rather than hiding or deprecating them.

Proposed vector API:

```rust
pub fn open(path: impl AsRef<Path>) -> Result<Store>;

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self>;
    pub fn collection(&self, name: impl Into<String>) -> CollectionHandle;
    pub fn list_collections(&self) -> Result<Vec<String>>;
}

impl CollectionHandle {
    pub fn upsert(&self, points: impl IntoIterator<Item = Point>)
        -> Result<WriteResult>;
    pub fn search(&self, vector: impl IntoIterator<Item = f32>, limit: usize)
        -> Result<Vec<ScoredPoint>>;
    pub fn get_ids(&self, ids: impl IntoIterator<Item = PointId>)
        -> Result<Vec<Record>>;
    pub fn delete_ids(&self, ids: impl IntoIterator<Item = PointId>)
        -> Result<WriteResult>;
    pub fn count(&self) -> Result<usize>;
    pub fn peek(&self, limit: usize) -> Result<Vec<Record>>;
    pub fn advanced(&self) -> Result<Collection>;
}
```

Names may change during implementation when rustdoc or call-site tests reveal a
more idiomatic choice, but the operation count and conceptual surface must not
grow.

### 2. Open-or-create behavior

`Store::open` follows explicit, safe rules:

1. Open the path when it is already a bare or non-bare Git repository.
2. Create a bare Git repository when the path does not exist.
3. Initialize an existing empty directory as a bare Git repository.
4. Reject an existing nonempty non-repository directory with an actionable
   error; never reinterpret or delete its contents.

The existing `Database::open`, `Database::init`, and `Database::init_bare`
retain their exact behavior.

### 3. Lazy collection creation and dimension inference

`Store::collection` validates only the collection name and returns a handle. A
read from a missing collection returns `CollectionNotFound`. The first nonempty
upsert:

1. verifies that all submitted vectors are nonempty and have one dimension;
2. creates the canonical collection directly from that batch using default
   configuration;
3. creates its commit and ref without force;
4. if another writer wins the ref race, opens the winner, validates its
   dimension, and retries the submitted upsert through existing compare-and-swap
   semantics.

This rule avoids a dimensionless persisted collection. Explicitly configured
empty collections continue to use `Database::create_collection`.

### 4. Search defaults and results

`CollectionHandle::search` constructs `Query::new(vector, limit)` and requests
payloads. It returns `QueryResult::points`, ordered exactly as today. Automatic
exact/approximate selection remains a property of the stored collection.

Users needing roots, execution mode, work counters, filters, vectors, or tuning
call `advanced()?.query(Query)` initially. A later convenience search builder is
allowed only if real examples show that it removes more concepts than it adds.

### 5. Metadata ergonomics

Add one fallible helper accepting any serializable JSON object, for example:

```rust
let point = Point::new("id", embedding)
    .with_metadata(serde_json::json!({"source": "guide.md"}))?;
```

Non-object JSON returns a focused validation error. The existing infallible
`with_payload(JsonObject)` method remains available.

### 6. CLI

The CLI will accept `--db` as the primary spelling while keeping `--repo` as a
hidden or documented compatibility alias. Mutating commands use open-or-create;
read commands do not create missing collections.

The first-use path supports:

- `upsert COLLECTION FILE` and `upsert COLLECTION -`;
- inline `--vector '1,0,0'` for a single point or query;
- `search` as the primary name, with `query` retained as an alias;
- automatic collection creation and dimension inference on first upsert;
- JSON output with stable field names and actionable errors.

Advanced collection configuration stays under `collection create`. History,
diff, validation, exact-mode overrides, and ANN tuning remain available but are
not shown in the first-use documentation.

### 7. Documentation information architecture

The README is capped at the information required to choose and try the project:

1. one-sentence product promise;
2. install;
3. persistent Rust quickstart;
4. short CLI quickstart;
5. four reasons to choose Git-native storage;
6. links to task-oriented guides and rustdoc.

Add tested guides for quickstart, persistence, filtering, history/Git transport,
and embeddings. Move format specifications, performance climbs, and internal
findings behind an “Internals and benchmarks” heading. Add an `llms.txt`-style
compact documentation map suitable for coding agents.

All code shown as runnable must be compiled or executed in CI. Examples should
use small deterministic vectors and temporary directories so they require no
network or model download.

### 8. Optional text layer

After the vector facade and documentation gates pass, perform a bounded spike on
an optional text layer. The user-facing contract is:

```rust
let docs = db.text_collection("docs", embedder)?;
docs.upsert_documents([Document::new("id", "A document about pineapple")])?;
let hits = docs.search_text("What grows in Hawaii?", 5)?;
```

The embedder has a stable model identity that is stored as the collection's
vector-space identity and checked on every write/query. The default `git-vdb`
dependency must not download models, require network access, or add a model
runtime. The spike will choose between an optional crate feature and one
first-party companion crate using these criteria:

- fewest steps in the quickstart;
- no default-core dependency cost;
- reproducible model identity and dimension;
- supported Rust/MSRV targets;
- package and documentation reliability.

If no candidate meets all four technical gates, document a small `Embedder`
trait and stop the goal at a working provider-independent adapter rather than
shipping a fragile default.

### 9. Compatibility and release

This work targets a new minor release while the crate is below 1.0. The facade
is additive. Existing advanced APIs and CLI spellings are preserved unless a
specific removal produces a strictly simpler interface and all repository call
sites are migrated in the same commit.

No storage-format change is expected. Differential tests will construct the same
logical collection through the facade and the existing APIs and require equal
root object IDs.

## Execution Plan and Evidence Rungs

### Rung 1: Golden-path contract

- Add compile-fail or integration-level call-site tests for the proposed Rust
  quickstart and safe open behavior.
- Record the current number of meaningful lines and required concepts.
- Stop if safe lazy creation requires a format change; revise the design before
  touching persistence.

**Evidence (2026-07-23):** `tests/store.rs` exercises the proposed persistent
open/collection/upsert/search call sequence. Inspection and a differential test
confirmed that the facade delegates to the existing builder and produces the
same format-v2 root as an explicitly configured advanced collection. No format
change is required.

### Rung 2: Core facade

- Implement `open`, `Store`, `CollectionHandle`, lazy first upsert, search, and
  common helpers.
- Prove equal roots against `Database`/`Collection` construction.
- Test missing reads, invalid dimensions, empty batches, existing repositories,
  nonempty non-repository directories, and first-write races.

**Evidence (2026-07-23):** the additive `Store` and `CollectionHandle` facade
implements safe open-or-create, inferred first writes, payload-returning search,
ID retrieval/deletion, count, peek, and advanced access. Tests cover missing
reads, empty and mixed dimensions, configuration mismatch without ref movement,
bare and non-bare repositories, preservation of occupied directories, reopening,
canonical-root equality, and concurrent first writers. The race test passed ten
consecutive runs. Full tests, clippy with denied warnings, denied-warning
rustdoc, and Rust 1.87 library/doctest checks pass.

### Rung 3: CLI first-use path

- Add auto-open/create, collection inference, stdin, inline vectors, and
  `search` naming.
- Preserve advanced commands and JSON output.
- Exercise the complete shell quickstart in an integration test.

**Evidence (2026-07-23):** the visible global option is `--db`, with `--repo`
retained as an alias. Mutating commands safely open or create the repository,
while a read against a missing path fails without creating it. Upsert accepts a
positional JSONL file, `-` for stdin, or an inline `--id`/`--vector` point;
`search` accepts inline comma-separated or JSON vectors and retains `query` as
an alias. The new end-to-end integration test starts from a nonexistent path and
exercises all three input forms, search, payload output, and count. The original
CLI/Git transport test passes unchanged through compatibility aliases.

### Rung 4: Documentation and examples

- Replace the README's primary example and reorganize links.
- Add task-oriented guides, agent documentation map, and small examples.
- Compile/test every public example and deny rustdoc warnings.

**Evidence (2026-07-23):** README and crate-level rustdoc now lead with the same
10-line persistent open/collection/upsert/search path. The README is limited to
installation, Rust and CLI first use, four Git-native benefits, guide links, and
an advanced/internals boundary. Task guides cover quickstart, persistence,
filtering, history/transport, and embeddings; `llms.txt` gives coding agents a
compact route map. Four matching examples compile under all-target tests and run
successfully as binaries. The four Rust guide snippets plus crate quickstart run
as five passing doctests, and rustdoc passes with warnings denied.

### Rung 5: Text-layer spike and implementation

- Evaluate the optional-feature and companion-crate approaches.
- Implement the smallest provider-independent layer plus one acceptable local
  default only if dependency, MSRV, reproducibility, and packaging gates pass.
- Add offline tests with a deterministic fake embedder regardless of provider.

**Evidence (2026-07-23):** a clean temporary-crate spike of `fastembed 5.17.3`
with default features disabled failed `cargo +1.87.0 check`: its exact
`ort 2.0.0-rc.12` dependency and `ort-sys` require Rust 1.88. This satisfies the
documented provider stop, so no model runtime or network dependency was added.
The provider-independent `Embedder`, `Document`, and `TextCollection` API ships
in the core crate without new dependencies. It embeds document batches and text
queries, retains source text in payloads, and persists/rechecks a nonempty model
identity as `vector_space`. Offline fake-embedder tests prove document search,
metadata retention, model mismatch rejection, and malformed-output rejection
without creating a collection. The guide is a doctest and has a matching
executable example.

### Rung 6: Release-quality verification

- Run formatting, all tests, clippy with warnings denied, rustdoc with warnings
  denied, MSRV checks, Nix checks, CLI smoke tests, and package dry-run.
- Run canonical-root differential tests for facade versus advanced APIs.
- Confirm the archive contains only intended user documentation and sources.
- Update the changelog and spec status only after every completion criterion is
  evidenced.

**Evidence (2026-07-23):** formatting, 34 unit/integration tests, all example
targets, denied-warning clippy, denied-warning rustdoc, six doctests, Rust 1.87
library/doctest checks, the clean-directory CLI smoke path, and all five native
Nix checks pass. The Nix source filter was extended to include task guides and
`llms.txt`, fixing a packaging-only missing-include failure found by this gate.
`cargo publish --dry-run --locked` packages 45 intended files (about 103 KiB
compressed), compiles the generated archive, and stops before upload as expected.
The final audit worktree is clean; the five implementation rungs are isolated in
preceding commits.

### Rung 7: Current Rust and first-party local embeddings

- Pin the repository to the current stable Rust toolchain and align the declared
  supported Rust version, CI, release instructions, and badge.
- Re-run the previously blocked FastEmbed spike without adding any dependency
  to the default feature set.
- Ship the provider only if its optional build, real local inference path,
  rustdoc, package archive, and supported CI platforms pass.

**Evidence (2026-07-23):** the official stable channel and `rustup` both report
Rust 1.97.1. `fastembed 5.17.3` now compiles with its Rustls download features
behind an empty-by-default `fastembed` feature. A real end-to-end example
downloaded and cached the default model, embedded two documents, persisted them,
embedded a query, and returned the expected document. `FastEmbedder` serializes
the provider's mutable inference session behind a mutex and persists a stable
model-variant/provider-version identity through the existing text layer. The
historical Rust 1.87 stop above remains as evidence of why the provider was not
included in the preceding release. Nix validates the default network-free
package because its sandbox cannot link a build-script-downloaded ONNX archive;
dedicated Linux and Windows CI jobs compile the opt-in feature, while local
macOS testing covers actual model download and inference. All five native Nix
checks then passed, as did the 46-file crate archive build and publish dry run.

## Stop Conditions

Work continues rung by rung until all goals are checked or one of these is
documented with reproducing evidence:

1. lazy first-write creation cannot be made race-safe without changing canonical
   storage or weakening compare-and-swap guarantees;
2. the simple facade produces different roots for equivalent inputs;
3. required dependencies cannot support the declared Rust version or current CI
   platforms;
4. the text provider requires network/model behavior in the default core crate;
5. a change causes a correctness or non-target performance regression that
   cannot be isolated within the rung.

A text-provider stop does not block completion of the vector facade. It changes
the text deliverable to a provider-independent adapter plus documented provider
integration contract.

## Validation Matrix

| Area | Required evidence |
| --- | --- |
| First use | Rust and CLI quickstarts run from clean temporary directories |
| Safety | Existing non-repository data is never overwritten |
| Canonical correctness | Facade and advanced APIs produce identical root IDs |
| Query correctness | Facade winners equal detailed-query winners |
| Compatibility | Format v1/v2 tests and historical operations pass |
| Concurrency | First-write race test has no silent ref overwrite |
| Documentation | Doctests and `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps` |
| Code quality | fmt, clippy `-D warnings`, full locked tests |
| Toolchains | Rust 1.97.1 library and doctests; Nix flake checks |
| Distribution | `cargo publish --dry-run --locked` and intended archive list |

## Open Questions

1. Resolved: keep `Store` and `CollectionHandle`. Call sites rely on inference,
   while the explicit names make rustdoc navigation unambiguous.
2. Resolved: `search` returns existing `ScoredPoint` values with payloads and no
   stored vectors. This avoids another result type while making metadata useful
   by default; detailed `query` retains all opt-in controls and statistics.
3. Resolved: use `--db` in first-use documentation because it names the user
   concept; retain `--repo` as a visible compatibility alias because Git-aware
   workflows still benefit from that precision.
4. Superseded in rung 7: the Rust 1.97.1 upgrade clears FastEmbed's `ort` gate.
   The provider now ships as an optional feature, while the default vector build
   retains no model, network, or ONNX dependency.

These questions may be resolved during their relevant rung. Decisions must be
recorded here before the goal is marked implemented.
