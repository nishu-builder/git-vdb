# Findings and benchmark status

## Current conclusion

The exact database, immutable Git roots, history, deterministic LSH layout, and
bounded lazy bucket search are implemented and tested. The version 1 LSH
defaults are provisional. No 100,000-point production-dimension run has yet
established recall@10 >= 0.95 while scoring <= 10% of points, so this repository
does not claim that acceptance target. Random-hyperplane LSH may require too
many multi-probes for that operating point; the next candidate should be a
history-independent, deterministically trained IVF-flat format with canonical
centroid assignment, introduced only as a new format version.

## Reproducible harness

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

The harness reports recall@1, recall@5, recall@10, median vectors-scored
fraction, build milliseconds, total exact/approximate query milliseconds, and
loose object bytes. Git object reads are not yet instrumented at the ODB layer;
query stats instead report canonical bucket and vector work. Packed size,
payload-only/vector/delete reuse matrices, hardware/revision capture, and the
full 100,000-point run remain outstanding. This omission is explicit because
inventing benchmark numbers would be worse than leaving the defaults
provisional.

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
