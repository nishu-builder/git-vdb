# git-vdb

[![CI](https://github.com/nishu-builder/git-vdb/actions/workflows/ci.yml/badge.svg)](https://github.com/nishu-builder/git-vdb/actions/workflows/ci.yml)
[![Supply chain](https://github.com/nishu-builder/git-vdb/actions/workflows/supply-chain.yml/badge.svg)](https://github.com/nishu-builder/git-vdb/actions/workflows/supply-chain.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

`git-vdb` is an embedded vector database whose immutable collection snapshots
are ordinary Git trees. It exposes collections, typed point IDs, dense vectors,
JSON payloads, upsert/get/delete/count, Qdrant-shaped filters, exact cosine
search, deterministic random-hyperplane LSH, history, validation, and structural
diffs. It has no server, daemon, network access, model inference, or telemetry.

Every successful mutation writes an immutable root tree, creates a commit
parented to the prior collection commit, and compare-and-swap updates
`refs/git-vdb/collections/<name>`. The root, unlike the commit, is deterministic:
the same configuration and point set produce the same object ID regardless of
input order, history, path, or bare versus non-bare repository layout.

The project is early-stage and welcomes focused feedback and contributions.
See [Contributing](CONTRIBUTING.md), [Security](SECURITY.md),
[Code of Conduct](CODE_OF_CONDUCT.md), [Support](SUPPORT.md),
[Roadmap](ROADMAP.md), and [Changelog](CHANGELOG.md).

## Install

Build the latest revision from source:

```sh
cargo install --git https://github.com/nishu-builder/git-vdb.git --locked
git-vdb --help
```

For reproducible production use, pin a commit or release tag rather than
installing a moving branch. Prebuilt binaries and a crates.io package are not
published yet.

## Install and develop

Requires stable Rust, libgit2's build prerequisites, and Git for the transport
integration tests.

```sh
cargo build --release
cargo test --all-targets --all-features
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps
```

## Rust API

```rust
use git_vdb::{CollectionConfig, Condition, Database, Distance, Filter, Point,
              Query};
use serde_json::json;

# fn main() -> git_vdb::Result<()> {
let db = Database::init("./vectors.git")?;
db.create_collection(
    "notes",
    CollectionConfig {
        dimension: 2,
        distance: Distance::Cosine,
        vector_space: Some("example/embedding-v1".into()),
        ..CollectionConfig::default()
    },
)?;

let collection = db.collection("notes")?;
collection.upsert(vec![Point {
    id: "note-1".into(),
    vector: vec![0.1, 0.2],
    payload: json!({"topic": "rust", "year": 2026})
        .as_object().unwrap().clone(),
}])?;

let result = collection.query(Query {
    vector: vec![0.1, 0.2],
    filter: Some(Filter::must([
        Condition::matches("topic", "rust"),
    ])),
    with_payload: true,
    ..Query::default()
})?;
println!("{} {}", result.root, result.points[0].score);
# Ok(())
# }
```

Mutation methods return `WriteResult { root, affected_points }`. Read results
always include the resolved root. `Collection::at(root_or_commit)` returns a
read-only historical view. `upsert_expect` and `delete_expect` add optimistic
writer protection; atomic ref matching still protects ordinary mutations from
concurrent lost updates.

Filters support scalar `match`, numeric `range`, `has_id`, nested groups, and
`must`/`should`/`must_not`. Dot-separated paths such as `author.team` traverse
nested JSON objects. Literal dots in keys are not escaped in format version 1.

## CLI

The global `--repo` flag takes precedence over `GIT_VDB_REPO`; otherwise the
current directory is used. stdout is one compact JSON value. Diagnostics go to
stderr.

```sh
git-vdb init ./vectors.git --bare
git-vdb --repo ./vectors.git collection create notes \
  --dimension 2 --distance cosine --vector-space example/embedding-v1

git-vdb --repo ./vectors.git upsert notes --input points.jsonl
git-vdb --repo ./vectors.git get notes --ids '"note-1"' --with-payload
git-vdb --repo ./vectors.git count notes --filter filter.json
git-vdb --repo ./vectors.git query notes --vector query.json --limit 10 \
  --with-payload
git-vdb --repo ./vectors.git history notes --limit 20
git-vdb --repo ./vectors.git validate notes --full
```

Point input is JSON Lines:

```json
{"id":"note-1","vector":[0.1,0.2],"payload":{"topic":"rust"}}
{"id":42,"vector":[0.3,0.4],"payload":{}}
```

CLI IDs are parsed as JSON when possible: `42` is an unsigned integer ID and
`"42"` is a string ID. An unquoted non-JSON token is treated as a string.
`query.json` may be a vector array or an object with a `vector` member.

Approximate mode can be forced with `--approximate` and bounded with `--probes`
and `--candidate-limit`; `--exact` forces the brute-force oracle. If neither is
given, the versioned full-scan threshold stored in the root chooses the mode.
Every query reports mode, buckets, candidates, vectors scored, and exhaustion.

## Storage and operational behavior

See [docs/format.md](docs/format.md) for the canonical tree, byte codecs, LSH
projection and probe order, and compatibility rules. See
[docs/findings.md](docs/findings.md) for current performance status and the
reproducible benchmark harness.

Git objects written before a failed ref update are unreachable and harmless.
Readers never update objects or refs. Deleting a collection deletes only its
named ref; normal Git reachability and garbage collection govern later object
retention. Repositories can be inspected, fetched, packed, and maintained with
stock Git.

## Current limits

Version 1 has one dense vector per point, cosine similarity only, no payload
index, and no sparse/hybrid search. Mutations reconstruct the logical tree from
the authoritative point set; Git still reuses identical blobs and subtrees, but
large write CPU cost is not yet proportional to the changed-point count. The
LSH defaults are deterministic rather than claimed production-tuned; the
100,000-point recall target has not yet been demonstrated. These limits are
recorded rather than hidden in canonical behavior.

## Community and project policy

Bug reports and scoped proposals belong in
[GitHub Issues](https://github.com/nishu-builder/git-vdb/issues). Please read
[SUPPORT.md](SUPPORT.md) before posting and use private vulnerability reporting
for security issues. Pull requests are expected to preserve the deterministic
root, immutable-history, atomic-ref, lazy-read, and exact-oracle invariants
described in [CONTRIBUTING.md](CONTRIBUTING.md).

Dependencies and GitHub Actions are monitored by Dependabot. CI runs formatting,
Clippy, documentation, and the test suite across Linux, macOS, and Windows;
cargo-deny checks advisories, licenses, duplicate versions, and dependency
sources.

Licensed under the [Apache License 2.0](LICENSE).
