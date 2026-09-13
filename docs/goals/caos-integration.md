# Semantic search over Caos snapshots

Status: planned. This document specifies implementation work; it does not
claim that a Caos adapter, worker image, or semantic-search tool exists yet.

## Invocation

From the repository root, give Codex this objective:

```text
/goal Read docs/goals/caos-integration.md completely and implement its required milestones and completion criteria. Deliver working semantic search over immutable Caos source snapshots, content-keyed embedding reuse, snapshot text APIs, and measured lazy object reads. Preserve format versions 1 and 2, the existing Rust/CLI/Python APIs, and unrelated work. Follow repository contribution rules, record reproducible evidence, and keep optional Caos integration dependencies outside the default core build. Publishing and merging require authorization from the initiating session; do not infer it from this reusable goal.
```

## Objective

Make git-vdb a reusable semantic index for immutable source snapshots in
[Metta-AI/caos](https://github.com/Metta-AI/caos). An agent or human should be
able to ask a natural-language question about a selected source snapshot and
receive ranked passages with exact file, line, and snapshot provenance.

Use git-vdb's deterministic snapshot engine as the index boundary. Let Caos
manage computation, caching, object transport, and result retention. Keep the
existing embedded database useful independently of Caos.

Complete milestones 1 through 5 below. Conversation memory is a separate
follow-up, not a reason to delay the source-search integration.

## Starting point and evidence

The initial assessment compared git-vdb
[`9edb4af` (0.4.0)](https://github.com/nishu-builder/git-vdb/tree/9edb4af748d7b11fc332de2eb6c0ac8e76a65b52)
with Caos
[`8e44b8f`](https://github.com/Metta-AI/caos/tree/8e44b8f51d97155c2287f47e6ae57eb4d4a23e20).
Recheck the relevant contracts before implementation and record the exact Caos
revision selected for the integration.

Existing capabilities and gaps:

- [SnapshotEngine and Snapshot](../snapshots.md) already build, apply, and query
  immutable Git trees without commits or refs. Directory materialization and
  import provide an initial transport bridge.
- [Document APIs](../embeddings.md) currently bind embedding providers to named
  collections. Equivalent ergonomics for immutable snapshots are needed.
- The current CLI indexes whitespace-based chunks using path-based IDs.
  Source retrieval needs explicit selection, preserved line spans, and content
  identity separate from file locations.
- Format-2 approximate search in `src/root_v2.rs::approximate_query` initially
  calls `read_all_points`. Bounded vectors scored does not imply bounded
  objects or bytes loaded.
- Caos supports repository-defined `caos-tools/<name>/.caos-expr` tools,
  pinned remote `:@@=` locators, and `std/flake-input-loader`.
- Caos's standard Cargo worker builds offline against its prepared dependency
  set; git-vdb's dependencies are not all in that set. Start with an
  independently packaged Nix worker image.

Read the pinned Caos
[tool contract](https://github.com/Metta-AI/caos/blob/8e44b8f51d97155c2287f47e6ae57eb4d4a23e20/SPEC.md#tools),
[consumer dependency support](https://github.com/Metta-AI/caos/blob/8e44b8f51d97155c2287f47e6ae57eb4d4a23e20/design/flake-inputs.md),
[recursive grep worker](https://github.com/Metta-AI/caos/blob/8e44b8f51d97155c2287f47e6ae57eb4d4a23e20/std/rgrep/src/main.rs),
and [conversation model](https://github.com/Metta-AI/caos/blob/8e44b8f51d97155c2287f47e6ae57eb4d4a23e20/design/chat.md).
Some design documents contain historical stages; verify current source and
tests instead of implementing a superseded stage.

## Architecture and boundaries

The intended data flow is:

```text
selected source tree + selection/chunking configuration
    -> content chunks + location manifest
chunks + pinned embedding configuration
    -> reusable embedding objects
embeddings + index configuration
    -> immutable git-vdb index tree
index tree + query embedding + filter/search parameters
    -> ranked passages with source provenance
```

Keep transport and worker orchestration in an optional `integrations/caos/`
package with its own dependencies and build outputs. The core crate may gain
general snapshot text APIs and an object-access interface, but must not require
a Caos server, provider account, worker runtime, or model download.

Use a result tree containing the index as a real subtree and a separate source
manifest. A textual hash alone does not make an index reachable for retention.
Keep application provenance outside the canonical collection layout when it
would violate format validation. Preserve the relationship between reusable
content and every file occurrence, including duplicate files.

Open or reuse a feature issue before implementing public API, CLI, or storage
behavior. The goal supplies the product direction; record concrete API choices
and compatibility implications in that issue and focused implementation PRs.

## Milestone 1: working search over one source snapshot

Build the smallest complete integration before optimizing it:

1. Pin a Caos revision and implement the consumer dependency entry points using
   current `.caos-expr`, typed locators, and declared dependencies.
2. Package a runnable worker image through Nix, reusing this repository's
   existing build setup. Keep embedding runtime/model packaging explicit.
3. Build an immutable index from a selected source tree and expose
   `caos-tools/semantic-search`. Support a query, path scope, and result limit.
   Expose historical selection through a documented snapshot argument or the
   containing Caos source reference, consistent with the current tool contract.
4. Start with the existing directory import/materialization API and a worker
   adapter calling the Rust snapshot engine. Record the eager import cost.
5. Return a readable answer plus structured results containing score, text,
   source tree ID, file blob ID, relative path, and line range. Keep source
   commit provenance where available without making it an embedding-cache key.

A deterministic fixture embedder is suitable for contract tests, but this
milestone also requires a real pinned text model and an actual Caos run.
Package model assets explicitly; initialization must work with outbound model
downloads disabled while preserving the worker's connection to Caos.

Honor the current tool result conventions: expected input failures become
readable result values, while unexpected failures fail the job. Both human and
agent invocation must expose the same answer and result identity for the same
effective request. Document any wrapper needed for structured output.

Acceptance: from a clean consumer checkout and a documented Caos stack, one
reproducible command sequence indexes a small repository and answers a question
with verifiable citations. The index is a ref-free git-vdb snapshot. Searching
does not modify the source tree.

## Milestone 2: content-keyed reuse and reliable provenance

Split ingestion into independently reusable computations:

- Identify chunks from file content and explicit chunker configuration. Preserve
  line boundaries and use stable chunk identities; do not bake an absolute
  checkout path or enclosing conversation commit into an embedding request.
- Include exact model weights, tokenizer, preprocessing, truncation,
  normalization, task/prompt mode, and adapter/runtime choices that affect
  vectors in the embedding identity. A model display name or library version
  alone is insufficient. Define compatible document/query embedding spaces.
- Separate file-location metadata from reusable embeddings. A rename should
  update provenance without recomputing unchanged embeddings. Identical files
  at different paths must remain distinct searchable occurrences.
- Narrow child requests to the content they process. A tool's outer request can
  include the selected source tree, but embedding jobs must not include the
  entire workspace merely because the caller supplied it.
- Select text files explicitly. Define handling for ignored/generated files,
  binaries, invalid UTF-8, symlinks, and Caos commit references. Keep output
  indexes and generated dependency mounts outside the selected corpus.
- Handle edits, additions, deletions, renames, empty files, and empty corpora.
  An index for a new snapshot must not retain deleted source occurrences.

Use bounded batching or subtree jobs where worker startup dominates; do not
assume one container per chunk is efficient. Reuse embeddings across snapshots,
then measure index-assembly work separately from embedding work.

Acceptance: record actual worker/cache traces showing zero document embeddings
recomputed for a repeat or a new query, reuse of unchanged file content after
an edit, and zero recomputation after a pure rename. Changes to effective model
or chunker configuration invalidate the affected cached work. Incremental index
assembly must equal a clean build from the same final points/configuration.

Keep determinism claims precise: index construction is deterministic for fixed
vector bytes. Validate model reproducibility separately; explicitly version or
isolate inference settings whose results differ across supported runtimes.

## Milestone 3: ergonomic text APIs for immutable snapshots

Design and implement a provider-independent way to build and query text
snapshots using the existing document/query concepts:

- Accept explicit embedding configuration and an immutable snapshot root.
- Check embedding-space compatibility before searching or modifying a snapshot.
- Support documents and metadata without requiring named collection refs.
- Expose batch query behavior and structured result/provenance information
  needed by the worker adapter.
- Share reusable text conversion logic with the current collection API.
- Add a CLI surface only where needed by the integration or general snapshot
  users; document its exact root, input, output, and error semantics.

Acceptance: examples and focused tests cover creation, reopen, query, mutation,
model mismatch, and historical queries. Rust, CLI, and Python compatibility
tests continue to pass, and snapshot operations still create no commits or refs.

## Milestone 4: measured lazy reads in fresh workers

Use milestone 1 as the unchanged eager baseline. Introduce the smallest
object-access boundary that supports both local Git objects and Caos objects
without requiring whole-index directory import for every query.

Read metadata, codebook, and selected postings first, then load the necessary
vector/ID shards. Fetch payloads for winners where filters permit; payload
filters may require earlier reads and those reads must be measured. Preserve
the existing exact/approximate ordering, filter semantics, and query bounds.

Format 2 has 64 hash-distributed point shards: a query may touch many shards
even when it scores few vectors. Do not equate candidate count with transfer
savings, claim row-level transfer within a blob, or promise sublinear I/O for
every query. Selective and broad queries both belong in the evaluation.

Acceptance:

- The Caos query path can operate without eagerly importing the entire index.
- Object/byte counters demonstrate omitted reads on a declared selective
  workload and honestly report broad-query behavior.
- The same roots, queries, filters, and parameters return the same ordered
  results as the eager baseline, including near ties and zero vectors.
- Cold worker latency, transfer bytes, and memory are measured alongside warm
  queries. Retain optimizations supported by repeatable evidence; explain
  negative results and remaining format limits.

Exact search remains a full scoring baseline. An on-disk format change is not
required for this goal; any proposal to change it needs a separate versioned
design and compatibility discussion.

## Milestone 5: reproducible validation and user documentation

Provide a small deterministic fixture suite and a checksum-pinned realistic
source corpus. Fix workloads before comparing implementations. Cover exact
search against an independent cosine oracle, approximate recall and result
counts, metadata filters, stale/deleted paths, duplicate content, historical
isolation, and model/configuration mismatch.

Record at least:

- git-vdb, Caos, model, tokenizer, toolchain, and image revisions;
- corpus/checksums, dimensions, chunking, filters, query parameters, and limits;
- CPU, architecture, memory, worker concurrency, and model execution settings;
- selection/chunking, embedding, assembly, startup, and query timings separately;
- cold and warm latency distributions, peak memory, objects/bytes transferred,
  vectors scored, and observed embedding cache hits/misses;
- repeat-query, new-query, one-file edit, rename, deletion, and model-change runs.

Use at least five repetitions for performance comparisons and report noise.
Treat a regression exceeding 5% and the observed noise as material; investigate
it and justify any accepted tradeoff. Do not tune workloads after seeing results
without rerunning both baselines. Keep bulky raw output outside Git and commit
a concise report with reproduction commands and artifact checksums.

Run `nix flake check`, relevant Rust/CLI/Python checks, and a real integration
smoke test against the pinned Caos stack. Include a forced fresh-request run
when validating execution/transport so a cached success cannot conceal broken
code, followed by unsalted reuse checks. Add a small CI check appropriate to
available infrastructure and document the explicit larger integration command.

Update the README, integration guide, examples, and a tracked implementation
report. Remove the planned-status wording only when the corresponding behavior
is implemented and verified. No benchmark claim may precede its evidence.

## Compatibility and completion

Preserve canonical bytes and meanings of format versions 1 and 2, deterministic
roots for identical point data, historical reads, exact cosine/tie semantics,
explicit approximate-query limits, and named-collection compare-and-swap.
Caches are derived accelerators and must not change persisted identity.

The required goal is complete only when:

- [ ] A clean consumer can run real semantic search through Caos with exact
      source citations and a documented pinned model.
- [ ] Repeated work, edits, renames, deletions, and configuration changes exhibit
      the specified reuse/invalidation behavior in recorded traces.
- [ ] Immutable text APIs are implemented, documented, and compatibility-tested.
- [ ] Caos queries use lazy object access with measured correctness and costs.
- [ ] Relevant checks pass and a reproducible report states limitations.
- [ ] The default embedded library still operates without Caos.
- [ ] Implementation changes are committed and published according to the
      initiating session's authorization.

A wrapper, fake-embedding demo, warm-only benchmark, or written plan alone does
not complete the implementation goal. If infrastructure prevents a required
gate, report the exact missing evidence and keep that criterion incomplete.

## Follow-up: selected conversation memory

After the required source-search goal, reuse the pipeline for explicitly
selected conversation notes, memories, and transcript passages. Caos represents
conversations and source references separately: unrelated conversation events
must not invalidate an unchanged code index. Preserve conversation and source
provenance, keep corpus selection explicit, and avoid indexing generated search
results recursively. Define retention and access scope before broadening beyond
the selected conversation.
