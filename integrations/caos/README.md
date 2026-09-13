# Semantic source search with Caos

This optional package connects git-vdb's immutable snapshots to Caos. It uses
Caos revision `8e44b8f51d97155c2287f47e6ae57eb4d4a23e20` and the CPU MiniLM
model recorded in [model.lock.json](model.lock.json). No Caos dependency or
model download is added to the default git-vdb crate.

## Run a search

Use a Linux Docker host with Nix and the pinned Caos host tools. A first build
downloads toolchains, runtime dependencies, and the explicitly pinned model;
leave several GB available for build and image storage. The worker performs
inference from packaged files.

```sh
nix build github:Metta-AI/caos/8e44b8f51d97155c2287f47e6ae57eb4d4a23e20#caos-tools -o /tmp/caos-tools
/tmp/caos-tools/bin/caosd up
# Run from the root of this checkout:
/tmp/caos-tools/bin/caos-cli run-tool semantic-search \
  --query='How do immutable snapshots preserve historical results?' \
  --path=src --limit=5
```

The command prints `tree <result-id>` on stdout and the readable answer on
stderr. The agent registers the same repository tool from its `help` argument;
the effective request and returned object identity are shared with human
invocation. No language-model provider is needed to invoke the tool directly.

Fetch structured output with
`caos-cli get <result-id> /tmp/semantic-result`. The result contains:

- `report`: passages, scores, paths and inclusive one-based line spans.
- `results.json`: exact text, scores, source tree and optional commit ID,
  file blob IDs, paths, line spans, index root, embedding identity, query
  counters and the object-read trace.
- `snapshot/index`: the actual canonical git-vdb subtree.
- `snapshot/manifest.json`: source occurrences, selection skips and model and
  chunking configuration.

The source tree is read without modification. Roots have no commits or named
collection refs. Returning the index as a subtree retains it through Caos's
result graph; a printed hash alone would not do that.

Pinned Caos reserves uppercase `FAILED` anywhere in a report as an error
marker. Successful reports render that word as `Failed` when it occurs in
source text or paths. Exact source bytes remain in `results.json`.

## Source selection and history

`path` defaults to `.`; it accepts a relative directory or file without
parent traversal. `limit` is 1–100, default 10. `chunk-chars` is 1–16384,
default 1200. Chunks preserve UTF-8 slices and end at that character limit or
24 lines. Long lines can span multiple chunks with the same line number.

The extension allowlist and excluded directory names are in
[src/lib.rs](src/lib.rs). Source code, Markdown, ordinary configuration and
selected extensionless files are included. Generated build directories,
dependency mounts, caches and index output directories are excluded. Empty or
whitespace-only files produce no chunks. Files over 256 KiB, NUL-containing
files and invalid UTF-8 are skipped after the blob has been fetched. Symlinks
and nested Caos commit references are skipped. An explicitly selected root
commit is resolved to its source tree, and its commit identity remains
provenance outside embedding requests.

Pass a containing immutable tree or commit using the ordinary typed Caos
`in` argument to search historical source. A path scope is interpreted inside
that selected source. A different checked-out source directory can be supplied
with `--in:@=/absolute/source`; the tool still comes from this repository.
Source selection and worker implementation are separate: changing this
integration or its declared core build dependencies versions the worker.

## Reuse and model identity

The orchestration creates one embedding request per distinct file blob with
bounded batches of 32 chunks. Its inputs are file content, chunking settings
and the pinned worker image. Paths and enclosing source/commit IDs belong to
a separate occurrence manifest. Identical files share embeddings while
remaining distinct results; a rename updates paths, and deletions disappear
from the newly assembled snapshot. Every index is assembled from the complete
current occurrence manifest, so there is no stale mutable index.

A new query reuses document embedding jobs and the assembled index. Query
embedding is a separate content-keyed job. Work whose input or effective
worker/model/chunker configuration changes receives a new Caos request
identity. Continuations release worker slots while waiting for child jobs.

Weights and tokenizer checksums, preprocessing, truncation, normalization,
provider, thread settings, adapter code and runtime versions identify the
vector space. Query and document prefixes are empty. Inference uses CPU,
one thread, sequential execution and float32 normalized output. Check
`/embed --describe` inside an image for its complete identity.
Index construction is deterministic for fixed vector bytes; model byte
reproducibility is only claimed for the execution environments actually tested.

## Selective reads and validation

The worker uses the general `SnapshotReader<SnapshotSource>` interface.
It loads metadata, the codebook and selected postings before candidate
ID/vector shards, and defers unfiltered payload reads until winners are known.
The result's `reads` counts logical blob requests/bytes; `objects` lists
materialized Caos objects and their hashes. These are not compressed network
byte counters. Exact search and broad filters can still visit every shard.

The eager directory-import path remains in `query_directory` for comparison.
The existing snapshot reader retains its decoded cache and remains appropriate
for repeated in-process queries. See the [implementation report](REPORT.md)
for measured costs, recall and limitations.

## Reproduce checks

Cheap checks need neither a Caos server nor model downloads:

```sh
cargo test --all-targets --locked
cargo test --doc --locked
cargo test --manifest-path integrations/caos/Cargo.toml --locked
GIT_VDB_BIN="$PWD/target/debug/git-vdb" PYTHONPATH=python/src \
  python3 -m unittest discover -s python/tests -v
nix flake check
```

For the deterministic object-read benchmark:

```sh
cargo build --release --example snapshot_read_benchmark --locked
target/release/examples/snapshot_read_benchmark fixture /tmp/vdb-fixture
python3 integrations/caos/benchmark.py \
  target/release/examples/snapshot_read_benchmark \
  /tmp/vdb-fixture /tmp/vdb-benchmark
```

The benchmark fixes four workloads, alternates reader order across five fresh
processes each, verifies exact results against an independent scalar cosine
oracle, and records five further warm queries per process. It reports recall,
logical bytes, latency distributions and Linux peak RSS. It does not drop the
host filesystem cache. Keep bulky raw results outside the repository.

The larger integration validation and pinned real-source corpus are documented
in the implementation report. They require a running Caos stack and the real
worker image; a cached successful answer alone is not an execution smoke test.
