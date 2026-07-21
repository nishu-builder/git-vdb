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

## Rung 2 hypothesis: retain immutable snapshot points

Rung 1 leaves every exact query reopening and decoding two or three blobs per
point. A `Snapshot` is immutable and already owns the lifetime of its object
database, while LanceDB's warm process retains table state. Keeping a decoded
point map with a snapshot should eliminate repeated object lookup on warm exact
queries. Snapshots returned by build/apply can retain the canonical map already
present during construction; snapshots opened by root can fill it after the
first successful exact read. Clones can safely share it.

Expected result: a large warm exact-query reduction with identical roots,
scores, ordering, filters, and query statistics. Tradeoff: live snapshots retain
roughly vector bytes plus IDs/payloads in memory. The named collection adapter,
one-shot `SnapshotEngine::query`, approximate search, and cold opened-snapshot
read remain uncached, keeping their costs separately visible.

### Result: accepted

Clustered raw results are in
`target/lancedb-results/smoke-20260721T232448Z`; uniform raw results are in
`target/lancedb-results/smoke-20260721T232709Z`. Exact IDs and scores still
agree with the oracle at k=1/10/100 and all selectivities. Roots, disk bytes,
approximate results, and named-adapter behavior are unchanged.

| Distribution | Rung 1 exact p50 | Rung 2 exact p50 | Reduction |
|---|---:|---:|---:|
| Clustered | 124,439.5 us | 241.5 us | 99.81% |
| Uniform | 73,475.0 us | 263.0 us | 99.64% |

A paired full-run RSS check reported 28,426,240 bytes for Rung 1 and 28,049,408
bytes for Rung 2. At this tier, the retained cache did not raise measured peak
RSS because the runner already keeps the source vectors alive. The full
clustered runner fell from 21.84 seconds to 14.75 seconds. The gain is far above
noise with no measured material regression, so Rung 2 is accepted.

## Rung 3 hypothesis: root-keyed named-adapter cache

The new named-adapter measurement at
`target/lancedb-results/smoke-20260721T233009Z` reports 72.099 ms exact p50,
versus 0.254 ms for snapshot core. Its profile and code path are the same
repeated loose-object decoding removed from immutable snapshots in Rung 2.

Hypothesis: collection handles can share a decoded-point cache keyed by the
resolved root. Every query still resolves the ref first; a changed root replaces
the cache, so writes through clones or other processes cannot return stale
points. Historical handles have independent immutable caches. Expected result
is a named-adapter warm exact gain comparable to Rung 2 without changing ref,
stale-writer, root, query, or approximate semantics. The memory-lifetime
tradeoff is limited to each live collection-handle family and measured again.

### Result: accepted after memory refinement

The first implementation used a `BTreeMap` cache and raised paired peak RSS by
5.2%, so it was rejected before commit. Cached points were changed to a compact
vector because scoring needs iteration, not key lookup. The resulting paired
RSS was 33,816,576 bytes versus the 32,391,168-byte baseline, a 4.4% increase
below the material threshold. This is a real memory tradeoff and remains
visible rather than being described as free.

Final raw results are in
`target/lancedb-results/smoke-20260721T233653Z`. Named exact p50 fell from
72,099 us to 270 us (99.63%), with exact IDs/scores unchanged. Named
approximate p50 was 9,126.5 us versus 9,085 us (0.46% slower), p95 and p99 both
improved, and approximate results were identical. Snapshot-core exact p50,
snapshot-core approximate p50, snapshot build p50, and named build p50 all
improved in the final three-repetition sample. A focused test confirms cache
invalidation after writes through a clone and a separately opened collection
handle. Rung 3 is accepted.
