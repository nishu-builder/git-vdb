# LanceDB performance continuation

## Fixed baseline and rules

This log continues `docs/benchmarks/lancedb-climb.md` under the objective in
`docs/goals/lancedb-performance.md`. The accepted starting point is the clean
100,000-point GloVe-25-angular run at revision `20776c5`, with summary checksum
`cbd20eb1837689becfbc5f5c6ba2d2a8a5aea0698571104b6d7a531f2ac69a82`.
LanceDB 0.34.0 remains pinned. Exact brute-force cosine search remains the
correctness oracle.

Format version 1 is immutable throughout Track A. An accepted Track-A change
must preserve canonical objects and roots, exact scores and ordering, and
version-1 approximate results at the same operating point. Baseline and
candidate samples use separate release executables, the same persisted snapshot
root, and interleaved execution on the retained `m6i.2xlarge` Linux box. A 5%
movement is material unless measured noise is larger.

## A1: attributable observability

The full harness intentionally measures end-to-end behavior, but its temporary
repositories prevent a profiler from reopening one fixed root. The benchmark-
only `lancedb_git_vdb_profile` example therefore has two commands: `build`
creates and retains a snapshot-core repository, while `query` reopens its exact
root, performs one unmeasured warmup, and records a batch of exact or approximate
queries. This separates build, exact, and approximate profiles without adding a
production code path or ref.

The first profiles will determine how much exact time remains in cosine scoring,
result construction/sorting, and cache access; and how much approximate time is
spent in Git bucket traversal versus candidate point decoding and scoring.
Loose-object and packed measurements use the same retained root so logical data
and query results cannot drift between conditions.

### A1 initial result

The retained Linux artifacts are under
`/home/ubuntu/git-vdb-performance/a1` on `nishadsingh-box-4`. The retained root
is `2a00e66b7976398bbf70daf9c9ff9c20dfc7d90f`, built from the exact pinned
100,000-point input. Its measured build time was 53.303 seconds, logical loose
bytes were 118,609,107, build peak RSS was 311,028 KiB, and `/usr/bin/time`
reported 2,855,688 filesystem-output blocks while creating the loose database.

Targeted Linux `perf` samples begin after the unmeasured warmup. In the exact
batch, 46.4% of sampled CPU was under `cosine` and 41.3% under the full stable
`sort_and_truncate`; this directly supports A2. The unchanged exact batch p50
was 32.528 ms in the profiling run and its process peak was 1,049,172 KiB. The
peak includes filling the immutable point cache before samples begin and is much
smaller than the full harness's combined multi-phase peak.

The unchanged approximate batch p50 was 213.784 ms, with 10,000 vectors scored
at the median and process peak RSS of 95,024 KiB. Of sampled approximate CPU,
86.7% was under `read_point_parts`, including 53.0% under `read_named_blob`;
bucket discovery under `find_bucket` was only 2.2%. Libgit2 `git_odb_read`
dominates the point-tree and blob stacks. This supports A4's first two steps:
reuse root-keyed decoded points before considering a complete in-memory postings
index. Logical ODB counts/bytes, mutation attribution, and pack/transfer effects
remain assigned to A5/A6 rather than being inferred from CPU percentages.

## A2 hypothesis: exact top-k work reduction

At the baseline, warm exact search constructs and clones one `ScoredPoint` per
eligible point, fully sorts all 100,000 results, and truncates to k=100. The first
candidate will select the best k with linear partitioning and sort only those k,
leaving scoring and result construction unchanged. This isolates the cost of
the O(n log n) full sort before attempting a bounded representation that avoids
loser allocations. Expected gain is at least 5% at 100,000 points with no root,
score, order, memory-lifetime, approximate-search, or public API change.

Norm hoisting/caching remains a separate later candidate because arithmetic
reuse and top-k work reduction must not be combined before either is measured.

### A2 result: accepted

The first candidate uses `select_nth_unstable_by` with the complete score/typed-ID
ordering, truncates to k, and stably orders only the selected k. Scoring and
construction of all candidates remain unchanged, so this rung isolates sorting
complexity. A focused unit test compares the selection with the old full sort at
every limit across equal scores, signed zero, string IDs, and unsigned IDs.

Five interleaved baseline/candidate pairs used separate release-with-debug-info
executables and the same retained root. Results and vectors-scored arrays were
identical after timing fields were removed.

| Pair | Baseline batch | Candidate batch | Candidate / baseline | Baseline p50 | Candidate p50 |
|---:|---:|---:|---:|---:|---:|
| 1 | 3.261 s | 2.061 s | 0.632 | 32.403 ms | 20.466 ms |
| 2 | 3.197 s | 2.039 s | 0.638 | 31.771 ms | 20.303 ms |
| 3 | 3.196 s | 2.091 s | 0.654 | 31.724 ms | 20.758 ms |
| 4 | 3.189 s | 2.064 s | 0.647 | 31.684 ms | 20.558 ms |
| 5 | 3.165 s | 2.040 s | 0.645 | 31.495 ms | 20.518 ms |

Every pair improved by 34.6% to 36.8%, far beyond the 5% materiality threshold.
Peak RSS fell slightly from about 1,049,270 KiB to 1,045,721 KiB. The unchanged
differential smoke run at `target/lancedb-results/smoke-20260722T035008Z` keeps
the exact oracle green at k=1/10/100 for both synthetic distributions and every
filter selectivity; its maximum `git-vdb` score error remains below `2.98e-8`.
No approximate code changed.

The clean candidate revision `f578b1b` completed that protocol on the retained
`m6i.2xlarge`. Raw results are under
`/home/ubuntu/git-vdb-lancedb-climb/target/lancedb-results/real-glove-25-angular-20260722T035425Z`;
the summary SHA-256 is
`591a9bb8a1c060eec6709efe1d5d2508028ab40eb96ff50b2717a0abc965cab2`.
All five raw git-vdb outputs are identical to the accepted baseline after timing
and resource fields are removed, including roots, exact and approximate ordered
results and score bits, filters, named-adapter results, and mutation roots.

| Metric | Accepted baseline | A2 candidate | Movement |
|---|---:|---:|---:|
| Exact query p50 | 26.768 ms | 16.385 ms | 38.8% lower |
| Exact throughput, concurrency 1 | 44.74 qps | 84.07 qps | 87.9% higher |
| Exact throughput, concurrency 4 | 157.0 qps | 275.3 qps | 75.3% higher |
| Build p50 | 53.445 s | 53.887 s | 0.8% higher |
| Approximate query p50 | 211.717 ms | 215.731 ms | 1.9% higher |
| Peak RSS p50 | 2.550 GB | 2.525 GB | 1.0% lower |

The candidate remains exact at k=1/10/100 with maximum score error
`2.98e-8`. At this machine and workload it narrows exact latency from 3.4x to
1.9x LanceDB, and its concurrency-4 exact throughput is 6.5% higher than the
same-run LanceDB median. Build, approximate search, storage, and mutation remain
structurally unchanged. Commit `f578b1b` is therefore graduated without a
format-version-1 byte or semantic change.

### A2 follow-ups: query norm rejected, bounded winners accepted pending graduation

The query-norm candidate hoists the query squared norm out of the point loop
without changing operation order for any score. Five interleaved retained-root
pairs produced candidate/baseline batch ratios of 0.960, 0.997, 0.975, 0.986,
and 0.966. The median gain is only 2.5%, below the 5% materiality threshold, so
the candidate is rejected and its production patch remains unapplied.

The bounded-winner candidate keeps borrowed point references and scores in a
size-k binary heap, then clones IDs, payloads, and vectors only for final
winners. Its complete score/typed-ID order is the heap order, including equal
scores and signed zero. Focused tests cover typed-ID ties, zero vectors,
filters, and limit zero. All five real-workload result sets and score bits are
identical to A2 after timing fields are removed.

| Pair | A2 batch | Bounded batch | Candidate / A2 | A2 p50 | Bounded p50 |
|---:|---:|---:|---:|---:|---:|
| 1 | 2.032 s | 1.137 s | 0.560 | 20.369 ms | 10.681 ms |
| 2 | 2.063 s | 1.159 s | 0.562 | 20.502 ms | 11.343 ms |
| 3 | 2.070 s | 1.028 s | 0.496 | 20.630 ms | 9.900 ms |
| 4 | 2.060 s | 1.041 s | 0.505 | 20.375 ms | 9.930 ms |
| 5 | 2.079 s | 1.008 s | 0.485 | 20.656 ms | 9.729 ms |

The median batch reduction is 49.5%. Process peak RSS falls from about
1,045,740 KiB to 189,474 KiB because losing IDs and result objects are no
longer retained. The differential smoke result at
`target/lancedb-results/smoke-20260722T050749Z` is semantically identical to
the unchanged implementation in both cases and all repetitions; its summary
SHA-256 is
`e13ebf8fc417cd5f908a073459209374bb5171c0ca923646cfc8d92dc468dc0b`.
Local commit `653f110` is accepted pending the pinned five-repetition graduation.

A new deterministic clustered 10,000 x 25 gate then exposed a pre-existing
near-tie mismatch that the declared 1,000-point and GloVe workloads do not
contain. Two distinct f64 cosine values can round to the same public f32 score;
the old exact comparator then used ID order while the independent f64 oracle
retained the true score order. Exact ranking now retains f64 internally and
casts only returned scores to f32. A focused fixture proves that the higher f64
score wins even when both public score bits are equal. The corrected 10,000-
point run at `target/lancedb-results/format2-10k-smoke-20260722T052242Z` passes
k=1/10/100 for unfiltered and every filter with maximum score error `2.98e-8`;
its summary SHA-256 is
`6a3b6f62dd0c26ab745ae4b1143ace72b6e03353ca516bfd37422e2938467ba7`.
All declared smoke and GloVe ordered results remain unchanged. The first remote
graduation attempt was stopped when this broader gate failed; only the corrected
candidate is eligible to restart.

## A4 hypothesis: reuse decoded points for approximate queries

The A1 approximate profile attributes 86.7% of sampled CPU to candidate point
decoding, while bucket discovery accounts for only 2.2%. The exact cache already
owns every decoded point for an immutable root, but the approximate path ignores
it and reopens each candidate point tree and its ID/vector/payload blobs on every
query. The first A4 candidate will add a compact, ephemeral point-tree-OID to
cache-index lookup and score candidates from the same root-scoped decoded point
view. Bucket discovery, candidate order and limits, hash/tree validation,
filtering, scoring, and returned values remain unchanged. The lookup is derived
only from the root's canonical point tree, shared across handle clones, and
discarded when a named collection root changes.

The expected warm-query gain is material because it removes repeated blob
decoding without adding an in-memory postings copy. Approximate-only use does
not instantiate the exact point cache and retains the unchanged ODB path. When
the exact view already exists, lookup construction time and RSS are explicit
costs: it traverses the canonical points tree once and retains one Git OID plus
an index per point. This candidate is rejected if ordered approximate IDs or
score bits change at any operating point, if construction erases the repeated-
query gain, or if an important memory or exact metric regresses by 5%.

### A4 result: accepted pending full-protocol graduation

Five interleaved pairs used the same retained root and first built the exact
view in both executables. The A4 candidate then constructed its point-tree OID
lookup during an unmeasured-but-reported approximate warmup. Baseline and
candidate ordered results, score bits, and vectors-scored arrays are identical
in every pair.

| Pair | Baseline batch | A4 batch | Candidate / baseline | Baseline p50 | A4 p50 |
|---:|---:|---:|---:|---:|---:|
| 1 | 20.678 s | 2.999 s | 0.145 | 230.940 ms | 30.759 ms |
| 2 | 20.643 s | 3.089 s | 0.150 | 229.997 ms | 32.027 ms |
| 3 | 20.486 s | 3.116 s | 0.152 | 228.943 ms | 32.077 ms |
| 4 | 21.129 s | 3.248 s | 0.154 | 236.086 ms | 33.302 ms |
| 5 | 21.194 s | 3.079 s | 0.145 | 236.956 ms | 31.414 ms |

The median batch reduction is 85.0%. Exact-view construction remains within
3.02--3.22 seconds in both executables. The candidate's first approximate
warmup costs 307--321 ms versus 249--265 ms because it constructs the lookup;
that one-time 56--72 ms cost is retained rather than hidden. Peak RSS rises
from about 267,805 KiB to 270,276 KiB, or 0.9%, for the OID/index array.
Approximate-only tests prove that an empty exact cache stays empty and follows
the unchanged ODB path. The local differential smoke result at
`target/lancedb-results/smoke-20260722T043327Z` has identical git-vdb semantics
for both cases and all three repetitions. Full 100,000-point graduation remains
required after the bounded exact candidate is graduated.

## Format-2 prototype: first canonical-layout smoke

The standalone prototype in `benchmarks/format2/` is not linked into the stable
API and cannot emit version 2 through a production writer. It materializes the
proposed sharded point blobs, deterministic training sample, IVF centroids, and
postings as real Git blobs and trees. Its first clustered 1,000 x 100 run is
`target/lancedb-results/format2-prototype-smoke-final.json`.

The prototype root is `b18a46d7a404d8ea1ad24fda69b0469d13afcf89` on both
Apple arm64 and the retained Linux x86_64 box. Reversing the complete input
order produces the same root on both platforms. The maintained independent
exact oracle agrees at k=1/10/100 with zero recorded score error. Approximate
recall is 1.000/1.000/0.9675 at k=1/10/100 and every unfiltered result-count
gate passes. Low-selectivity filtered ANN still underfills, so those timings
are not improvement claims.

The base root contains 2,687 unique blobs and 534,241 logical blob bytes; its
loose repository occupies 685,544 file bytes. The comparable version-1 smoke
run reports 1,551,757 bytes, so the candidate layout is 55.8% smaller at this
tier before packing. A 1% vector update has reverse-input root equality and
shares 2,675/2,687 blobs and 475,253/534,241 logical blob bytes with the base
root.

The observed prototype build was 1.060 seconds, but this is not yet an accepted
performance claim: it was not interleaved with a same-executable baseline and
its Git writer is a benchmark subprocess implementation. The changed-shard Git
serializer produces the same `0d635d97abefa21cbfdb96e749859af433f0022d`
mutation root as a clean serializer and as reversed input on both architectures.
In one smoke run it wrote the changed root in 112 ms versus 410 ms for a clean
write; including a 3.7 ms full index recomputation, the mutation took 118 ms.
The serializer is incremental, but centroid training and point assignment still
recompute globally. The prototype's concurrency 1/4 paths execute real query
batches. External measurement reports 60.6 MB peak RSS, versus 35.9 MB for the
Rust version-1 smoke runner, so the prototype does not claim a memory
improvement.

Explicit Git maintenance retains a readable base root. At this small tier the
packed repository is 739,528 bytes, 7.9% larger than its 685,544-byte loose
form; mirror clone and one-root fetch are 728,567 and 728,493 bytes. Pack headers
and repository metadata dominate the small payload, so this is retained as
negative evidence rather than extrapolated into a packing win. The prototype
still needs 10,000- and 100,000-point runs, external phase RSS, and incremental
centroid/assignment maintenance.
