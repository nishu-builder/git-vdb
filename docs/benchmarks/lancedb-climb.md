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

## Rung 4 hypothesis: reuse unchanged point trees during apply

At the final Rung 3 baseline, 1% upsert/delete p50 is 1.009/0.902 seconds,
10% is 1.305/1.113 seconds, and 100% upsert is 2.084 seconds. The weak scaling
with changed fraction and the build profile's Git object checks show that apply
rewrites point blobs/trees for all surviving points even though Git ultimately
deduplicates their bytes.

Hypothesis: retain the existing point-tree OID while reading the previous root
and pass it through canonical root construction for IDs not upserted. Deletes
need no new point tree; upserted IDs are always re-encoded and validated. Index
and root trees are still rebuilt from the complete final set, preserving exact
format-version-1 bytes and roots. Expected gain is largest for 1%/10% mutation,
with no read/query or clean-build effect and no memory-lifetime tradeoff.

### Result: accepted

The whole-harness run at
`target/lancedb-results/smoke-20260721T234013Z` was invalid for performance
acceptance because unrelated clean-build time rose about 60%. Interleaved
focused A/B runs used the same raw data and runner with only the selected
mutation fraction enabled.

For 1% mutations, all five pairs improved. Median candidate/baseline ratios
were 0.522 for upsert (47.8% reduction) and 0.673 for delete (32.7%
reduction). For 10% mutations, all three pairs improved; median ratios were
0.900 for upsert (10.0% reduction) and 0.877 for delete (12.3% reduction).
The existing incremental-versus-clean-rebuild test kept the exact root gate
green, as did the full suite and Clippy. Clean build, reads, queries, and stored
bytes do not use the new reuse input. Rung 4 is accepted.

## Rung 5 hypothesis: update only changed canonical tree paths

Rung 4 still recomputes every LSH signature and rebuilds all point/index trees.
The remaining 1% mutation cost is about 0.6 seconds in paired candidate runs.
Version 1's path layout makes a narrower update possible: each typed ID has one
point hash prefix and one bucket per table. Applying the net batch to builders
seeded from the previous root can rewrite only affected point prefixes,
signature buckets, signature prefixes, tables, and their ancestors.

Hypothesis: track the original value on first mutation of each ID, discard net
no-ops, write new point trees only for final upserts, and patch the old/new LSH
signatures. Clean rebuild equivalence remains the oracle for exact root bytes.
Expected gain is proportional to the changed fraction. The tradeoff is more
update code, bounded by focused root-equivalence tests across payload-only,
vector, insert, delete, filter-delete, and canceling batches.

### Result: accepted

Five interleaved 1% pairs all improved: median candidate/baseline ratios were
0.380 for upsert (62.0% reduction) and 0.393 for delete (60.7% reduction).
Three interleaved 10% pairs all improved: median ratios were 0.693 for upsert
(30.7% reduction) and 0.640 for delete (36.0% reduction). The final unchanged
harness run is `target/lancedb-results/smoke-20260721T235226Z`; its p50 values
are 195.077/185.667 ms at 1% and 667.297/577.659 ms at 10%.

Exact oracle checks, filtered exact checks, approximate outputs, roots, and all
tests are green. New focused tests cover a mixed insert/vector/payload/delete-ID/
delete-filter batch, deletion to an empty root, insertion from empty, full index
validation, and a delete-then-restore net no-op. Rung 5 is accepted.

## Rung 6 hypothesis: avoid full reads for ID-only batches

After Rung 5, 1% upsert/delete still costs about 0.19 seconds and 100% delete
has a similar floor. `SnapshotEngine::apply` decodes the complete collection
before it knows which IDs change. ID-only upsert/delete batches can instead load
the old point directly from its deterministic hash path on first touch, track
presence transitions for the metadata count, and feed the same net changes to
Rung 5's tree updater. Filter deletes still require a full scan and keep the
existing path.

Expected result: ID-only cost approaches work proportional to touched points;
filter semantics and mixed filter batches remain unchanged. The returned root
is still checked against clean rebuilds. A fast-path result does not eagerly
retain a full decoded snapshot cache; its first later exact query fills that
cache normally, trading eager memory for lower mutation latency without a
public semantic change.

### Result: accepted

All five interleaved 1% pairs improved. Median candidate/baseline ratios were
0.516 for upsert (48.4% reduction) and 0.530 for delete (47.0% reduction).
All three 10% pairs improved, with median ratios 0.902 for upsert (9.8%
reduction) and 0.838 for delete (16.2% reduction). The final unchanged harness
run is `target/lancedb-results/smoke-20260721T235841Z`: p50 is 120.062/100.608
ms at 1% and 547.794/435.666 ms at 10%.

The clean-root comparisons, full validation, exact oracle, filters, cache
invalidation, full suite, and Clippy remain green. Filter batches still use the
full-scan path. Rung 6 is accepted.

## Pinned real-data baseline

`real-smoke.json` ran three cold repetitions over the first 10,000 train vectors
and 12 test vectors from the checksum-pinned ANN-Benchmarks GloVe-25-angular
file. Raw output is
`target/lancedb-results/real-smoke-20260722T000038Z`.

| Metric | git-vdb snapshot core | LanceDB 0.34.0 |
|---|---:|---:|
| Build p50 | 25.429 s | 0.124 s |
| Exact query p50 | 0.733 ms | 1.436 ms |
| Approximate query p50 | 102.140 ms | 1.038 ms |
| Approximate recall@1 | 0.917 | 1.000 |
| Approximate recall@10 | 0.817 | 0.867 |
| Approximate recall@100 | 0.600 | 0.705 |
| On-disk bytes/point | 1,299.9 | 206.6 |
| 1% upsert p50 | 0.969 s | 0.019 s |
| 1% delete p50 | 0.800 s | 0.003 s |

Both exact engines agree with the independent oracle at k=1/10/100. Maximum
score error is `2.98e-8` for git-vdb and `1.95e-7` for LanceDB. The git-vdb
named adapter exact p50 is 0.792 ms and resolves the collection ref on every
query. git-vdb therefore wins this warm exact latency tier while retaining
deterministic roots and Git history semantics that LanceDB does not provide.
LanceDB wins build, mutation, storage, and approximate search decisively.

At 0.1% filter selectivity, both approximate configurations fail result-count
and recall gates. git-vdb also fails k=100 result count at 10% selectivity and
both fail it at 1%. These approximate timings are reported as negative evidence,
not accepted performance. Version 1 random-hyperplane LSH remains provisional.

## Pinned 100,000-point real-data protocol

The complete `real-glove-25.json` protocol ran five cold repetitions over
100,000 train vectors and 100 test vectors on an EC2 `m6i.2xlarge`: Intel Xeon
Platinum 8375C, 8 logical CPUs, 30.8 GiB RAM, x86_64 Linux. Revision `20776c5`
records actual concurrency-1/4 throughput, externally measured peak RSS, and
Linux CPU metadata. Raw output is
`target/lancedb-results/real-glove-25-angular-20260722T013255Z`; the SHA-256 of
`summary.json` is
`cbd20eb1837689becfbc5f5c6ba2d2a8a5aea0698571104b6d7a531f2ac69a82`.

| Metric | git-vdb snapshot core | LanceDB 0.34.0 |
|---|---:|---:|
| Build p50 | 53.445 s | 2.560 s |
| Exact query p50 | 26.768 ms | 7.959 ms |
| Approximate query p50 | 211.717 ms | 1.939 ms |
| Exact throughput, concurrency 1 | 45.1 qps | 123.1 qps |
| Exact throughput, concurrency 4 | 157.0 qps | 264.3 qps |
| Approximate throughput, concurrency 1 | 5.27 qps | 519.3 qps |
| Approximate throughput, concurrency 4 | 19.39 qps | 891.0 qps |
| Approximate recall@1 | 0.970 | 0.950 |
| Approximate recall@10 | 0.904 | 0.902 |
| Approximate recall@100 | 0.793 | 0.835 |
| Peak RSS p50 | 2.550 GB | 0.957 GB |
| On-disk bytes/point | 1,186.1 | 206.1 |
| 1% upsert p50 | 4.144 s | 7.255 ms |
| 1% delete p50 | 3.944 s | 2.750 ms |

Both engines and the named adapter agree with the independent exact oracle at
k=1/10/100 for unfiltered and all filtered searches. Maximum exact score error
is `2.98e-8` for git-vdb and `2.18e-7` for LanceDB. git-vdb's named-adapter
exact throughput is 32.5 qps at concurrency 1 and 125.0 qps at concurrency 4;
its historical-read p50 is 0.297 ms.

The full-size result reverses the 10,000-point Apple-silicon exact-latency win:
LanceDB is faster and smaller for every measured resource and timing category
at 100,000 points. git-vdb has slightly higher unfiltered recall@1 and recall@10,
while LanceDB has higher recall@100. At 1% selectivity both engines fail the
approximate k=100 result-count gate. At 0.1%, git-vdb fails k=10/100 and LanceDB
fails k=1/10/100. Those filtered ANN operating points remain negative evidence.

## Stop condition and next rung

The reproducible harness, synthetic smoke baseline, pinned 10,000- and
100,000-point real baselines, exact correctness gates, and six accepted
improvement rungs are complete. The current small safe frontier has been
exhausted: exact query now scores cached points in memory, and ID mutation work
is proportional to directly loaded IDs and changed Git tree paths. Remaining
profiles point to fundamental version-1 costs:
12 canonical LSH entries per point, loose Git object creation/tree validation,
and an ANN recall frontier that cannot be fixed by timing-only patches.

The former local-storage stop was cleared by the 200 GiB remote box. The full
protocol peaked with more than 166 GiB still free and is accepted evidence. A
material ANN improvement still requires a reviewed format-2 design; it must not
silently change format version 1.

The highest-leverage next rung is a version-2 proposal for a deterministic,
history-independent IVF-flat layout, evaluated first on the pinned 100,000-point
real dataset. For version 1, the next observability rung is ODB-level object/
byte read instrumentation plus packed-repository and clone/fetch transfer
measurements; the current harness reports vectors scored, loose disk bytes, and
logical structural reuse, process concurrency, and peak RSS but does not claim
the remaining ODB or transfer counters.

## Reproduction and accepted commits

```sh
nix flake check --print-build-logs
nix develop -c uv run --frozen --project benchmarks/lancedb \
  python benchmarks/lancedb/harness.py \
  --workload benchmarks/lancedb/workloads/smoke.json
nix develop -c uv run --frozen --project benchmarks/lancedb \
  python benchmarks/lancedb/harness.py \
  --workload benchmarks/lancedb/workloads/real-smoke.json
nix develop -c uv run --frozen --project benchmarks/lancedb \
  python benchmarks/lancedb/harness.py \
  --workload benchmarks/lancedb/workloads/real-glove-25.json
```

Accepted local commits, in order:

- `289a3c2` — pinned differential harness;
- `64ea328` — lazy exact object decoding;
- `4969c3b` — immutable snapshot exact cache;
- `a84bb2f` — named-adapter measurements;
- `dc5f7e9` — root-keyed named collection cache;
- `938ac31` — unchanged point-tree reuse;
- `5b54f67` — changed-path-only canonical tree updates;
- `e1d2809` — touched-ID-only mutation reads;
- `b5781e2` — pinned real-data smoke workload;
- `20776c5` — executed concurrency, peak RSS, and Linux CPU metadata.

Rejected or invalid evidence retained in this log: non-interleaved whole-run
comparisons whose unrelated build/approximate variance exceeded 5%, and the
first named-cache representation whose 5.2% RSS increase crossed the material
threshold. The first 100,000-point run at revision `7cb59dd`, retained in
`real-glove-25-angular-20260722T002333Z`, was rejected because its workload
declared concurrency 1/4 without executing it and did not capture peak RSS or
Linux CPU identity. No test or workload was weakened to obtain an accepted
result.
