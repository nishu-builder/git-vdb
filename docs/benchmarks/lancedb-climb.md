# LanceDB climb log

## Protocol

The maintained harness, dependency lock, workload definitions, and exact
reproduction command live in `benchmarks/lancedb/`. Raw output and downloaded
datasets live below `target/` and are not committed. Format version 1 is the
normative database format throughout this log.

## Baseline: clustered 1,000 x 100 smoke case

Run on 2026-07-21 at revision `ea67e83bde4c9bc195694f53d8266f8fb6f09b19`
plus the uncommitted harness, Apple M4 Pro, 48 GiB RAM, arm64 macOS 15.7.3,
Rust 1.91.0, LanceDB 0.34.0. Raw result directory:
`target/lancedb-results/smoke-20260721T230642Z`.

Both exact engines agreed with the independent oracle at k=1/10/100. Maximum
score error was `2.98e-8` for git-vdb and `2.11e-7` for LanceDB. Median warm
exact query time was 110.770 ms for git-vdb and 0.991 ms for LanceDB. Median
approximate query time was 9.294 ms and 0.866 ms respectively, but git-vdb
returned too few results at k=100 and both declared ANN operating points
returned too few filtered results at low selectivity. Those approximate timings
therefore do not pass the correctness gate and are not improvement claims.

The baseline has high build variance because the first run populated filesystem
caches; build claims require more controlled repetitions. Mutation cost in
git-vdb is roughly proportional to the complete collection rather than the
changed fraction, which remains a separate measured frontier.

## Rung 1 hypothesis: exact scan object decoding

Before production edits, a 10-second `sample` profile of the unchanged
10,000 x 100 exact-query workload captured 8,448 of 8,521 main-thread samples
under `query_root`; 5,071 samples ended in `open(2)`. The stacks were dominated
by `read_stored_points`, Git loose-object lookup, and `read_named_blob`. The
exact path currently materializes a `BTreeMap` of complete `Point` values and
decodes every payload even when neither a filter nor payload output is
requested. The approximate path already reads point parts lazily.

Hypothesis: traverse canonical point-tree entries directly and reuse the
existing lazy part decoder in exact search. This should remove one payload blob
read and JSON canonicalization per scored point, plus the intermediate full
point map, without changing scores, ordering, roots, bytes, or public APIs.
Expected gain is above the smoke run's observed query noise. Tradeoff: an
unfiltered query that does not request payload will no longer incidentally
validate payload bytes, matching the existing approximate-query behavior;
`validate(full=true)`, get, filtered query, and payload-returning query continue
to validate them.

### Result: accepted

The complete post-change smoke run is
`target/lancedb-results/smoke-20260721T231517Z`. Exact oracle agreement remained
true for both distributions, every filter selectivity, and k=1/10/100. Stored
roots and the approximate path were unchanged. Because whole-harness clustered
runs showed substantial unrelated filesystem variance, acceptance used five
interleaved pairs of separately compiled baseline and candidate binaries on the
same deterministic 1,000 x 100 clustered workload with 60 exact queries.

| Pair | Baseline total | Candidate total | Candidate / baseline |
|---:|---:|---:|---:|
| 1 | 6,001 ms | 4,750 ms | 0.792 |
| 2 | 8,697 ms | 4,666 ms | 0.537 |
| 3 | 6,275 ms | 4,714 ms | 0.751 |
| 4 | 8,786 ms | 4,781 ms | 0.544 |
| 5 | 6,358 ms | 5,125 ms | 0.806 |

Every pair improved. The median paired ratio was 0.751, a 24.9% reduction that
exceeds both the 5% materiality threshold and observed candidate variance. No
important metric is structurally affected outside exact query. Rung 1 is
accepted.
