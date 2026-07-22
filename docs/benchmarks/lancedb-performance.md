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

### A2 result: accepted pending full-protocol graduation

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
No approximate code changed. Final acceptance requires the full repository gate
and the pinned five-repetition 100,000-point protocol at the candidate revision.

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
