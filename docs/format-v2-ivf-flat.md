# Historical format version 2 proposal: deterministic sharded IVF-flat

Status: implemented and superseded by the normative
[format-version-2 specification](format-v2.md). This document preserves the
original design rationale and acceptance plan; proposal details such as the
12-bit shard candidate and opt-in rollout are historical, not current behavior.
The production acceptance evidence is in
[`docs/benchmarks/lancedb-performance.md`](benchmarks/lancedb-performance.md).

## Motivation and evidence

The accepted 100,000-point GloVe-25 comparison shows that format version 1 is
correct but structurally expensive relative to LanceDB 0.34.0: build is 20.9x
slower, approximate query is 109.2x slower, peak RSS is 2.7x higher, loose bytes
per point are 5.8x higher, and 1% ID mutation is hundreds of times slower.
Profiles attribute warm approximate time primarily to repeatedly decoding Git
point trees and blobs, while build and mutation profiles expose the cost of
canonical object creation and validation.

Version 1 stores three point blobs plus a point tree and twelve LSH references
per point. Incremental updates now touch only changed canonical paths, so the
remaining write/storage floor cannot be removed by another traversal shortcut.
Version 2 should reduce object count and make warm search operate on compact
arrays while retaining Git-native identity and history semantics.

## Goals

- Identical logical points and configuration produce the same root regardless
  of insertion order, mutation history, repository packing, process, or
  supported platform.
- Exact cosine results retain the version-1 score tolerance and canonical typed
  ID tie order.
- ANN construction and results are deterministic and independently measurable
  against exact search.
- Point storage uses thousands rather than hundreds of thousands of Git objects
  at the 100,000-point tier.
- Reads can form a compact immutable in-memory view without reconstructing one
  heap allocation per stored field.
- Small mutations rewrite deterministic shards and affected postings rather than
  one path per table per point.
- Version 1 remains fully readable and writable. Version 2 is opt-in until its
  format, migration, and operational behavior are approved.

Non-goals are matching LanceDB on every metric, hiding Git maintenance work, or
making HNSW-style mutable graph construction deterministic after the fact.

## Proposed canonical root

```text
meta.json
points/
  ids/<shard-3hex>.bin
  payloads/<shard-3hex>.bin
  vectors/<shard-3hex>.f32le
index/ivf-flat-v2/
  codebook.bin
  sample.bin
  postings/<centroid-4hex>.bin
```

The initial prototype uses 12 ID-hash shard bits, producing at most 4,096 point
shards. Empty shard files do not exist. Paths and Git modes remain canonical;
Git's tree encoder supplies bytewise name ordering. Shard bits, centroid count,
training sample limit, iteration count, distance, and numeric rules are stored
in `meta.json` and are never read from environment variables.

The shard of a point is the first 12 bits of the version-1 typed-ID SHA-256.
Rows within a shard ascend by the complete typed canonical ID bytes. This makes
placement independent of input order and prevents an insertion from shifting
unrelated shards. Twelve bits are a prototype operating point: at 100,000
points a uniform hash distribution averages about 24 rows per shard. The
benchmark must compare other fixed shard widths before the value becomes
normative.

### Point blobs

Every binary file begins with an eight-byte magic, a little-endian format minor
version, row count, and checked section lengths. Integers are fixed-width
little-endian unless explicitly variable-length. Decoders reject trailing
bytes, non-canonical offsets, invalid UTF-8, and arithmetic overflow.

`ids/<shard>.bin` stores a row-offset table followed by canonical typed IDs.
String IDs contain a type byte, length, and unmodified UTF-8; unsigned IDs
contain a type byte and u64 value. `payloads/<shard>.bin` stores offsets followed
by the same canonical JSON bytes required by version 1. `vectors/<shard>.f32le`
stores dimension and row-major finite IEEE-754 binary32 components. The three
files have identical row counts and row order.

Separating IDs, payloads, and vectors allows unfiltered vector search to avoid
payload decoding, filtered search to map rows without one Git tree per point,
and winner materialization to decode only selected payloads. The prototype must
also measure a combined ID/payload blob; the split is accepted only if its
query benefit exceeds its extra object and tree cost.

At 100,000 points the proposed point layout has at most 12,288 data blobs plus
fanout trees, versus per-point trees/blobs and 1.2 million logical LSH entries in
version 1. Git packing is physical and does not change any canonical blob ID.

## Deterministic codebook construction

IVF-flat is the leading hypothesis because its persisted structure is a
codebook plus one posting per point, and its query work has an explicit probe
count. It is preferable to HNSW for the first prototype because graph topology
is sensitive to insertion order, concurrent construction, and neighbor-update
history.

### Training sample

The sample contains at most 8,192 points with the smallest complete typed-ID
hashes, ordered by hash then canonical typed ID. `sample.bin` records those IDs
and source vector hashes so validation can prove the selected sample. Sample
selection is a pure function of the final point set.

The centroid count is stored explicitly. The initial benchmark candidate is
`clamp(round(sqrt(point_count)), 1, 4096)`. Clean build and incremental apply
must derive the same value. Crossing a count boundary legitimately rebuilds the
codebook and postings and must be reported as a discontinuity.

### Initialization and Lloyd iterations

Initialization selects actual sample vectors at evenly spaced positions in the
SHA-256 digest order after applying a format-domain-separated permutation. It
does not use process randomness. Assignment ties choose the lowest centroid
index. Accumulation visits sample rows in canonical order and dimensions in
ascending order. A fixed iteration count is used; convergence-based early exit
is not permitted in the canonical builder. Empty centroids retain their
previous vector for that iteration.

Centroid components are rounded to binary32 after every iteration and the exact
bits are persisted. No BLAS, compiler fast-math, reassociation, parallel
reduction, or implicit fused multiply-add is allowed in canonical construction.
The normative implementation must define each f64 multiply/add and square-root
operation and pass cross-platform golden tests on x86_64 and arm64 before this
algorithm can be approved. If native IEEE operations cannot prove bit equality,
the prototype must use a software-defined arithmetic routine or reject trained
IVF as the canonical persisted index. Runtime query scoring may be optimized
only if it retains the declared result tolerance and ordering.

This numeric gate is deliberately strict: deterministic seeding alone does not
make floating-point training a deterministic Git format.

### Incremental implications

The codebook is always the function of the final deterministic sample, never of
the collection's history. A mutation that does not change sample membership or
a sampled vector can reuse `codebook.bin`. An insertion/deletion that changes
the sample, or an update to a sampled vector, recomputes the codebook and all
assignments. The 8,192-point limit bounds that training work but does not hide
the possible full-postings rewrite.

The benchmark must report sample-stable and sample-changing mutations
separately. Freezing a codebook from the original build is rejected because an
incremental result would then differ from a clean rebuild of the same final
point set.

## Assignments and postings

Every point is assigned to the closest centroid under the specified canonical
cosine comparison; ties choose the lower centroid index. Each
`postings/<centroid>.bin` contains entries ordered by point shard then row. An
entry is a 12-bit shard number plus a row number wide enough for the declared
shard bound. It does not duplicate vectors, IDs, or payloads.

One posting blob per centroid makes a query read only the selected lists and
keeps total logical postings proportional to point count. A small mutation can
rewrite many centroid blobs when changed points are widely distributed, but the
total posting payload is compact. The prototype must compare this layout with
centroid-and-ID-prefix posting shards if 1% mutations rewrite a material
fraction of total posting bytes.

Query processing loads the codebook, ranks centroids with canonical tie order,
reads the requested posting lists, resolves rows through the immutable point
shards, applies filters, scores exact vectors, sorts by score then typed ID, and
truncates to k. It reports probed centroids, discovered candidates, vectors
scored, and whether probe/candidate limits were exhausted. Adaptive probing or
exact fallback is a named query policy, not an invisible change to the base
operating point.

## Exact search and the in-memory view

Exact search iterates vector shards and uses a bounded top-k winner structure.
The immutable root-keyed view may retain contiguous vectors, compact row-to-ID
metadata, point norms, payload offsets, codebook, and postings. Cache contents
are derived solely from the resolved root, shared by handle clones, and replaced
after a root change. They are never part of root identity.

Cold cache construction, retained bytes, warm latency, concurrency throughput,
and eviction lifetime are separate metrics. The stable API continues to return
owned IDs/payloads/vectors. A cache is an acceleration layer, not authority;
full validation reads canonical objects.

## Mutation and structural sharing

Apply resolves each touched ID to its hash shard. For a sample-stable ID batch:

1. Decode only affected point shards and the affected old posting blobs.
2. Apply ordered mutations and rewrite each changed point shard canonically.
3. Reassign changed vectors using the unchanged codebook.
4. Rewrite posting blobs for old and new centroid memberships.
5. Rebuild only changed Git tree paths and canonical metadata.

Filter deletion still scans payload shards unless a separately versioned filter
index is introduced. A sample-changing mutation rebuilds the codebook and all
postings but can still reuse unchanged point shards. A 100% mutation naturally
rewrites all point content.

The 12-bit shard proposal trades object count for write amplification: 1% of
uniform random IDs is expected to touch a material minority of point shards.
The prototype must report unique shards and logical bytes rewritten at
1%/10%/100%, not merely changed point count. Incremental apply is accepted only
when its final root equals a clean rebuild byte for byte.

## Validation, corruption, and resource bounds

Basic validation checks canonical `meta.json`, required trees, file headers,
lengths, row-count agreement, finite vectors, sorted unique IDs, correct shard
selection, centroid bounds, and posting order. Full validation additionally:

- recomputes the deterministic sample and codebook;
- recomputes every assignment and posting list;
- verifies point count, uniqueness, and all referenced rows;
- rejects missing, duplicate, or extra postings;
- validates canonical payload JSON and vector-space metadata.

Readers check all sizes before allocation and enforce configured bounds for
dimension, shard rows, centroids, sample count, probes, candidates, and output
limit. Corruption returns an error and never repairs or rewrites a root during a
read.

## Git history, failure atomicity, and transfer

The named adapter remains a commit/ref layer above immutable roots. A version-2
write creates all blobs and trees before creating a commit, then advances the
collection ref with the existing compare-and-swap transaction. Orphan objects
after failure are acceptable Git garbage; a failed write cannot advance the
head. Historical commits dispatch readers by the root's format version.

Diff first compares metadata and shard/posting object IDs. It decodes only
changed point shards to report added/removed/changed typed IDs. Materialization
exports the canonical tree. Import recomputes its tree ID. Stock Git packing,
clone, fetch, and garbage collection remain valid; benchmark claims must report
both loose logical layout and packed/transfer bytes.

## Compatibility and migration

- Stable collection and snapshot creation now emits format version 2
  unconditionally; there is no opt-in mode or experimental fork.
- Readers dispatch on `meta.json.format_version`; version-1 interpretation does
  not change.
- Existing version-1 roots remain readable, fully validatable, and mutable;
  mutation preserves version 1 and never rewrites the old root.
- Root IDs are intentionally different across versions, while exact results and
  logical point contents retain the same semantics.
- No bulk conversion or downgrade command is shipped. The repository had no
  external users when v2 became the default, so a migration surface would add
  unneeded compatibility complexity; immutable v1 roots remain the fallback.

## Prototype and acceptance plan

The first implementation belongs in a standalone benchmark path. It must not be
reachable from stable collection creation. In order:

1. Prove binary codec round trips, corrupt-input rejection, and golden hashes.
2. Prove codebook/assignment equality across insertion permutations, clean and
   incremental builds, separate processes, x86_64 Linux, and arm64 macOS.
3. Run synthetic smoke and the pinned 10,000-point GloVe workload.
4. Measure shard width, sample size, centroid count, and probe count without
   changing the accepted query set after seeing candidate timings.
5. Freeze an operating point and run the unchanged five-repetition 100,000-point
   protocol on the same `m6i.2xlarge` class.

Required outputs are exact agreement, ANN recall/result count, p50/p95/p99
build/query/mutation time, concurrency 1/4 throughput, phase and process RSS,
logical and packed bytes, objects/point, structural reuse, bytes rewritten,
clone/fetch transfer, historical reads, and full validation.

A prototype advances to production review only if it materially improves
several structural categories without an unexplained correctness, determinism,
memory, transfer, or mutation regression. LanceDB parity is not required.

## Rejected shortcuts and open review decisions

Rejected before prototype:

- insertion-order or thread-scheduling-dependent training;
- freezing the original codebook when a clean rebuild would choose another;
- runtime environment variables that alter canonical construction;
- HNSW as the first persisted candidate without a canonical graph proof;
- native BLAS/fast-math in canonical training;
- silently emitting v2 through the v1 default or rewriting old roots;
- reporting warm cache speed without cache-build time and retained RSS.

The prototype must resolve these review decisions with evidence: shard width;
split versus combined point blobs; deterministic arithmetic implementation;
sample limit; centroid-count rule; posting sharding; cache lifetime/budget; and
migration/default policy. Failure to prove cross-platform codebook and root
equality is a stop condition for trained canonical IVF, not a reason to weaken
the deterministic-root invariant.
