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
