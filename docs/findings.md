# Findings and benchmark status

## Current conclusion

Format version 2 is the production default. It combines deterministic 64-way
point sharding with a history-independent IVF-flat index and retains immutable
Git roots, exact search, history, validation, and explicit query-work bounds.
The unchanged five-repetition 100,000-point GloVe-25 protocol establishes
recall@10 of 0.969 while scoring at most 10% of points, exact agreement with the
independent f64 oracle, complete filtered result counts, and identical roots
across repetitions and the tested arm64/x86_64 platforms.

Against production format version 1 on the same `m6i.2xlarge`, v2 is 22.0x
faster to build, 6.3x faster for median approximate queries, 11.3x smaller per
point, 15.1% lower in median peak RSS, and faster at every 1%/10%/100% mutation
case. Recall improves at k=1/10/100 and exact p50 improves slightly. This clears
the documented no-regression gate, so there is no opt-in v2 path: all new roots
use v2. Canonical v1 roots remain readable, validatable, and mutable.

Relative to same-run LanceDB 0.34.0, v2 now builds slightly faster, uses about
half the bytes per point, uses less peak memory, has near-equal exact latency,
higher ANN recall, and higher concurrency-4 ANN throughput. The remaining gaps
are 2.4x slower single-query ANN latency and much slower Git-native mutations.
Full evidence and artifact hashes are in
[`docs/benchmarks/lancedb-performance.md`](benchmarks/lancedb-performance.md).

## Reproducible harness

The maintained LanceDB differential harness and current accepted evidence are
documented in `benchmarks/lancedb/README.md` and
`docs/benchmarks/lancedb-climb.md`. It pins LanceDB 0.34.0 and dependencies,
includes synthetic uniform/clustered and checksum-pinned GloVe-25-angular data,
and supersedes the older single-engine example below for comparison claims.

`examples/benchmark.rs` generates deterministic clustered data with seed
`0x676974766462626d`, builds a temporary bare repository, compares approximate
results to the exact oracle, and emits JSON containing recall, scored fractions,
roots, and timings. Generated repositories live in a temporary directory and
are not committed.

```sh
cargo run --release --example benchmark -- 1000 768 100
cargo run --release --example benchmark -- 10000 768 100
cargo run --release --example benchmark -- 100000 768 100
```

Arguments are point count, dimension, and query count. The standard matrix is
1,000, 10,000, and 100,000 points at dimension 768, 100 deterministic queries,
32 clusters, and the format defaults documented in `format.md`.

The older example reports recall@1, recall@5, recall@10, median vectors-scored
fraction, build milliseconds, total exact/approximate query milliseconds, and
loose object bytes. The maintained differential harness additionally captures
hardware and revision metadata, exact-oracle agreement, filtered recall, peak
RSS, concurrency-1/4 throughput, mutation fractions, and the full pinned
100,000-point GloVe-25 run. Git object reads are not yet instrumented at the ODB
layer; packed size and clone/fetch transfer measurements also remain
outstanding. This omission is explicit because inventing benchmark numbers
would be worse than leaving the defaults provisional.

## Recorded run

The first production-dimension tier was run on 2026-07-21 on an Apple M4 Pro
running Darwin 24.6.0, using a release build from the initial uncommitted
implementation and the documented defaults. It used 1,000 points, dimension
768, 32 clusters, 20 queries, and seed `0x676974766462626d`.

| Metric | Result |
|---|---:|
| Build | 2,419 ms |
| Exact queries, total | 2,170 ms |
| Approximate queries, total | 695 ms |
| recall@1 | 1.000 |
| recall@5 | 1.000 |
| recall@10 | 1.000 |
| median vectors scored | 5.5% |

This small clustered run is a harness smoke test, not evidence for the
100,000-point target. The exact query path currently reads canonical point
objects afresh per query, so its timing includes repeated Git object decoding.

## Correctness evidence

The automated suite currently covers canonical typed IDs and vector bytes,
nested filters, exact cosine/tie ordering, point operations, full index
validation, deterministic roots across input permutation and bare/non-bare
repositories, historical roots, stale compare-and-swap, approximate work
bounds, CLI JSON behavior, stock Git tree inspection, fetch, repack, and
garbage collection. The suite also confirms that a clean rebuild of a resulting
point set has the same root as incremental mutation.
