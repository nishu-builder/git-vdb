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

### A2 follow-ups: query norm rejected, bounded winners accepted

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
Local commit `653f110` is accepted. Its pinned graduation is reported with A4
below because both cache-only changes can be proven against the same unchanged
version-1 output.

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

### A3 result: compact exact representation not pursued

The bounded-winner path reduced exact retained state from one owned result per
eligible point to k borrowed winners. The final retained-root profile attributes
93.9% of sampled CPU to the inlined exact scan and cosine loop, while heap push
and pop are only 0.06% and 0.19%. More importantly, the graduated full protocol
puts exact p50 within 17% of LanceDB, concurrency-4 throughput 33% above LanceDB,
and whole-process peak RSS 8% below LanceDB. A second persisted or contiguous
point representation would add cache construction, invalidation, and memory
budget obligations to pursue a now-small exact-only gap. A3 is therefore not an
applicable production change at this frontier. SIMD or changed accumulation
order remains outside the version-1 semantic gate.

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

### A4 result: accepted

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
for both cases and all three repetitions.

The combined bounded-winner, f64-ranking, and decoded-point-reuse revision then
completed five clean repetitions on the retained `m6i.2xlarge`. The raw result
is
`/home/ubuntu/git-vdb-a4/target/lancedb-results/real-glove-25-angular-20260722T052722Z`
and the summary SHA-256 is
`ca825902727b77bedd8e2485010455e4f73a1cab9ff675c2897cfb87a33d42bb`.
The remote clean revision is `76c278c`; the equivalent local production source
is commit `4f8a81d`. All five git-vdb outputs are identical to A2 after timing
and resource fields are removed, including canonical roots, ordered exact and
approximate IDs and f32 score bits, filters, named-adapter results, and mutation
roots.

| Metric | Starting v1 | Graduated v1 | Same-run LanceDB | Graduated gap |
|---|---:|---:|---:|---:|
| Build p50 | 53.445 s | 53.382 s | 2.504 s | 21.3x slower |
| Exact query p50 | 26.768 ms | 9.361 ms | 8.007 ms | 1.17x slower |
| Approximate query p50 | 211.717 ms | 28.715 ms | 1.882 ms | 15.3x slower |
| Exact throughput, concurrency 1 | 44.74 qps | 114.67 qps | 119.59 qps | 4.1% lower |
| Exact throughput, concurrency 4 | 157.0 qps | 354.14 qps | 265.84 qps | 33.2% higher |
| Approximate throughput, concurrency 1 | not retained | 36.73 qps | 515.80 qps | 14.0x lower |
| Approximate throughput, concurrency 4 | 19.39 qps | 148.60 qps | 895.29 qps | 6.0x lower |
| Peak RSS p50 | 2.550 GB | 0.894 GB | 0.976 GB | 8.4% lower |
| Bytes per point | 1,186.1 | 1,186.1 | 206.1 | 5.8x higher |
| 1% upsert p50 | 4.144 s | 4.261 s | 6.551 ms | 650x slower |

Exact IDs agree with the independent f64 oracle at k=1/10/100 for unfiltered
and every filter, with maximum score error `2.98e-8`. ANN recall remains
0.970/0.904/0.793 at k=1/10/100, and the previously declared low-selectivity
underfill remains visible. Build, stored bytes, and mutation structure are
unchanged. The combined result makes exact search and memory competitive while
leaving approximate search and all write/storage metrics as the honest
version-1 frontier.

After A4, a profile of `approximate-after-exact` no longer contains candidate
point decoding. On a packed retained root, 20.8% of samples are zlib inflate,
8.2% SHA-1 hashing, 8.5% byte comparison, and 20.8% the remaining query loop;
the Git stacks resolve canonical bucket trees. A complete ephemeral postings
copy could remove some of that work, but would duplicate 1.2 million LSH
references, introduce a second potentially large root-keyed cache, and still
leave build, mutation, and transfer unchanged. It is deferred at this stop
boundary in favor of reviewing the measured compact format-2 layout; it is not
claimed as an exhausted or impossible optimization.

## A5: loose versus packed operation

The retained A5 repository uses the same 100,000-point root
`2a00e66b7976398bbf70daf9c9ff9c20dfc7d90f` and the final logical format-1
objects. Raw artifacts are under `/home/ubuntu/git-vdb-performance/a5`.
`git count-objects` reports 344,893 loose objects occupying 1.36 GiB of
allocated filesystem space. The repository's apparent file bytes are
143,709,436, close to but distinct from the harness's 118,609,107 logical data
bytes because Git metadata and filesystem allocation answer different
questions. A mirror clone through `file://` packs those objects into one
119.42 MiB pack; the packed bare repository is 125,288,838 apparent bytes.

| Operation | Loose source | Packed source | Packed movement |
|---|---:|---:|---:|
| Allocated object storage | 1.36 GiB | 119.42 MiB pack | about 91% lower |
| Mirror clone elapsed | 50.98 s | 4.58 s | 11.1x faster |
| One-snapshot fetch elapsed | 20.99 s | 5.65 s | 3.7x faster |
| Resulting mirror bytes | 125,288,839 | 125,288,839 | identical |
| Resulting fetch-repository bytes | 125,292,880 | 125,292,881 | metadata noise only |
| Cold exact cache plus first query | 182.997 s | 1.847 s | 99.1x faster |
| Warm exact p50 in that process | 21.043 ms | 22.613 ms | no pack benefit |
| Cold approximate-only first query | 17.601 s | 0.535 s | 32.9x faster |
| Approximate-only 100-query batch | 103.489 s | 9.204 s | 11.2x faster |

The cold exact comparison used the immediately preceding profile helper, whose
selection path predates the final bounded heap; cache decoding and physical ODB
access are unchanged, while the warm exact latency is not a frontier claim.
The clean five-repetition table above is the authoritative final exact result.
Approximate-only deliberately does not instantiate the exact view, so its
batch measures direct ODB behavior rather than A4's after-exact cache reuse.

Packing is therefore a strong explicit maintenance operation for cold reopen,
clone, fetch, and direct ODB queries. It does not accelerate initial build,
because the measured writer first emitted all canonical loose objects. It also
does not improve warm exact scanning after the immutable point view exists.
Automatic foreground packing is not proposed: the measured repack after adding
mutation roots took 7.22 seconds and 220,744 KiB peak RSS, work that must remain
visible or run as separately requested/background maintenance.

## A6: first-mutation write floor

The controlled cold 1% mutation produced identical upsert root
`dc75fdf03a2b9948966328b81d0d5248e14363e4` and delete root
`271ee8a850566e75cbd135197cfb8ce24348b66e` from loose and packed bases.
Packing accelerates base-object reads, but both paths create exactly 20,795 new
canonical loose objects occupying 165,280 KiB according to
`git count-objects`.

| 1% mutation phase | Loose base | Packed base | Movement |
|---|---:|---:|---:|
| Upsert | 16.213 s | 4.686 s | 3.5x faster |
| Delete | 4.063 s | 3.842 s | 5.4% faster |
| Whole command elapsed | 22.29 s | 8.71 s | 2.6x faster |
| New loose objects | 20,795 | 20,795 | identical |

The loose upsert includes cold inflation of many base objects; the packed result
returns to approximately the warm full-harness floor of 4.261 seconds rather
than approaching LanceDB's 6.551 ms. In 40,000 mutation profile samples, zlib
inflate/deflate and related trees account for more than 44%, and SHA-1 hashing
for more than 7%; Git tree-builder and allocation work follows. This is direct
evidence that required canonical Git decompression, hashing, compression, and
object/tree creation dominate the remaining version-1 write. No cached-
existence or parallel-signature patch was attempted without a profile-supported
non-Git target.

After refs were added for the base, upsert, and delete roots, `git gc` packed
all 365,688 reachable objects into one 121.46 MiB pack. `git fsck --full
--strict` passed. Full validation remained green for the base (100,000 points),
upsert (100,000), and historical delete (99,000) roots in 4.85, 4.88, and 4.77
seconds. Packing and garbage collection therefore preserve immutable root
meaning and historical readability.

## Format-2 prototype: first canonical-layout smoke

The standalone prototype in `benchmarks/format2/` is not linked into the stable
API and cannot emit version 2 through a production writer. It materializes the
proposed sharded point blobs, deterministic training sample, IVF centroids, and
postings as real Git blobs and trees. Its first clustered 1,000 x 100 run is
`target/lancedb-results/format2-prototype-smoke-final.json`.

This initial 1,024-sample prototype root is
`b18a46d7a404d8ea1ad24fda69b0469d13afcf89` on both Apple arm64 and the
retained Linux x86_64 box. Reversing the complete input order produces the same
root on both platforms. The maintained independent
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
negative evidence rather than extrapolated into a packing win. That first-stage
prototype still lacked larger runs, external RSS, and mutation-scale evidence;
the next section supplies those measurements while retaining the incremental
centroid/assignment limitation.

### Format-2 prototype: 10,000- and 100,000-point arm64 evidence

The prototype training sample now matches the proposed 8,192-point bound. It
also measures 1%/10%/100% vector mutations and a separate sample-stable 1%
mutation. Every mutation uses changed-shard serialization and independently
requires equality with clean serialization and reversed input. Centroid
training and assignment still recompute globally, so these are not production
incremental-apply claims.

On the deterministic clustered 10,000 x 25 fixture, the arm64 root is
`52622e40ad08ed7d0c64a2a24e641e1e3d2a472d`. Independent exact agreement is
green at k=1/10/100 with zero score error; unfiltered ANN recall and result count
are 1.000 at all three k values. Build took 3.973 seconds, exact p50 was 0.848
ms, approximate p50 was 0.115 ms, and external peak RSS was 180.9 MB. The root
contains 11,049 blobs and 1,910,322 logical bytes; the loose repository is
2,263,472 bytes versus 11,532,892 bytes for version 1. Packing remains negative
at this tier: 2,463,277 bytes, 8.8% larger than loose.

The 10,000-point sample-changing mutations took 0.376/0.619/1.583 seconds at
1%/10%/100%. The sample-stable 1% mutation took 0.364 seconds, reused its
codebook, and shared 10,956/11,049 blobs and 1,880,034/1,910,322 logical bytes.
Low-selectivity filtered ANN still underfills; that negative result is retained.

The unchanged 100,000-point GloVe vectors produce arm64 root
`2b53af15a7ac8e69f2df79adb1c23bfc679b4b36`. Independent exact agreement is
green at k=1/10/100 with zero score error. The single-run prototype comparison
is intentionally labeled provisional rather than interleaved production
evidence:

| Metric | Graduated version 1 | Format-2 prototype |
|---|---:|---:|
| Build | 53.382 s | 5.182 s |
| Exact query p50 | 9.361 ms | 9.279 ms |
| Approximate query p50 | 28.715 ms | 0.503 ms |
| Approximate recall k=1/10/100 | 0.970/0.904/0.793 | 0.910/0.863/0.778 |
| Process peak RSS | 893.9 MB | 497.4 MB |
| Loose repository bytes | 118,609,107 | 13,349,532 |
| Loose Git objects / prototype unique blobs | 344,893 | 12,607 |
| 1% vector upsert | 4.261 s | 1.198 s |
| 10% vector upsert | 13.330 s | 2.251 s |
| 100% vector upsert | 97.690 s | 2.432 s |

Packed size is 12,186,865 bytes; mirror clone and one-root fetch are 12,136,192
and 12,136,118 bytes. The sample-stable 1% mutation takes 1.119 seconds and
reuses its codebook; the sample-changing 1% mutation takes 1.198 seconds. The
prototype materially improves several structural categories, but ANN recall is
lower than version 1 at all three k values and filtered underfill remains. The
Linux x86_64 replay produced the identical base root, the identical
sample-stable root, and the identical 1%/10%/100% sample-changing roots. Every
root also equals clean serialization and reversed input on both architectures.
The x86_64 process peak was 445,420 KiB. This clears the prototype's two tested-
platform determinism gate; additional NumPy/BLAS/compiler combinations remain
external gates rather than assumed portability.

The retained arm64 reports are
`target/lancedb-results/format2-prototype-10k-arm64-final.json` and
`target/lancedb-results/format2-prototype-glove-100k-arm64-final.json`, with
SHA-256 values
`250a47bd63bd07fcc82809fdb5155ce63b02e99fb2eba80ceb1f60dab872345b` and
`ca8ba01d39a1ff75b093f4fc04433d7ac87696d95e626f5a530f1c00d1c58b8e`.
The x86_64 report is
`/home/ubuntu/git-vdb-performance/format2-x86-100k.json`, SHA-256
`6e45211a9af4d3981a36839c5445f89ed275464d15257c77455bd6ab41d0f1f8`.

## Decision boundary and remaining uncertainty

The documented stop condition is met: every Track-A rung has an outcome,
version-1 packed behavior is characterized, the write/storage floor is
attributed to required canonical Git work, and the isolated format-2 proposal
and prototype have enough evidence for product review before production
compatibility and migration obligations are created.

Accepted production hypotheses are partial top-k selection, bounded borrowed
winners, f64 internal ranking for exact near ties, and reuse of the immutable
decoded point view by approximate search. Together they reduce exact p50 65%,
approximate p50 86%, and peak RSS 65% from the accepted starting point without
changing a version-1 root or declared query result. Explicit Git packing is
accepted as an operational recommendation for cold reopen and transfer, not as
a hidden foreground build optimization.

Rejected or deferred hypotheses are also material evidence. Query-norm hoisting
improved the median by only 2.5% and remains unapplied. A3's second compact exact
representation is not justified after reaching LanceDB-class exact behavior.
Automatic foreground packing would add a measured 7.22-second maintenance
phase. A full version-1 postings cache remains plausible but is deferred because
it duplicates the structurally expensive LSH layout and cannot improve writes
or storage. The format-2 prototype is not accepted production code: its ANN
recall is lower at the frozen probe setting, filtered underfill persists,
centroid/assignment work still recomputes globally, historical named-adapter
reads are absent, and its 100,000-point timing is a single-run standalone Python
prototype rather than five interleaved production repetitions.

The highest-leverage next decision is whether to authorize an opt-in production
format-2 implementation and migration boundary. Before that authorization, a
review must choose the acceptable ANN recall/probe operating point and the
normative deterministic arithmetic strategy. If approved, the first production
gate is cross-platform golden codecs/codebooks, full validation and historical
named reads, followed by the unchanged five-repetition `m6i.2xlarge` protocol.
If format 2 is not approved, the narrower alternative is an explicitly bounded
version-1 postings cache with construction time and retained-memory limits; it
cannot address the measured build, mutation, or storage floor.

## Reproduction, artifacts, and commits

The combined full-run summary is retained at the A4 path above. The A5/A6 raw
directory is `/home/ubuntu/git-vdb-performance/a5`; an aggregate SHA-256 over
the named build, clone/fetch timing, query, mutation, profile, GC, and validation
artifacts is
`7fd2af717455e3070581663555a63a76e221270bc11e0e612bac949fcae08069`.
Representative commands are:

```sh
nix flake check --print-build-logs
nix develop -c uv run --frozen --project benchmarks/lancedb \
  python benchmarks/lancedb/harness.py \
  --workload benchmarks/lancedb/workloads/smoke.json
nix develop -c uv run --frozen --project benchmarks/lancedb \
  python benchmarks/lancedb/harness.py \
  --workload benchmarks/lancedb/workloads/real-glove-25.json

nix develop -c cargo build --release --example lancedb_git_vdb_profile
target/release/examples/lancedb_git_vdb_profile \
  build CASE/run.json RETAINED.git build.json
target/release/examples/lancedb_git_vdb_profile \
  query CASE/run.json RETAINED.git build.json approximate-after-exact query.json
target/release/examples/lancedb_git_vdb_profile \
  mutate CASE/run.json RETAINED.git build.json 0.01 mutate.json
git clone --mirror file://$PWD/RETAINED.git PACKED.git
git --git-dir=PACKED.git gc

nix develop -c uv run --frozen --project benchmarks/lancedb \
  python benchmarks/format2/prototype.py \
  --run-spec CASE/run.json \
  --output target/lancedb-results/format2-prototype.json
```

Accepted local commits, in chronological order, are:

- `6952bdc` — retained-root query profiler;
- `f578b1b` — partial exact top-k selection;
- `95263ff` — deterministic sharded IVF-flat proposal;
- `5f35dca` — isolated mutation and validation profiles;
- `22163a8` — standalone deterministic format-2 prototype;
- `abd135f` — format-2 concurrency measurements;
- `1370f3f` — format-2 Git packing and transfer measurements;
- `6d4612a` — format-2 storage smoke evidence;
- `54404a8` — changed-shard format-2 serialization;
- `653f110` — bounded exact winner state;
- `4f8a81d` — decoded-point reuse and exact near-tie correction;
- `4b4b6d7` — 1%/10%/100% format-2 mutation measurements.

Raw benchmark output, datasets, and generated repositories remain untracked.
No format-version-1 bytes, default format selection, tests, workloads, or
correctness thresholds were weakened, and no branch was pushed.
