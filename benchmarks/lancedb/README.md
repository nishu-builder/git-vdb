# LanceDB differential harness

This harness compares the common embedded cosine-search subset of `git-vdb`
and LanceDB 0.34.0 without treating LanceDB as the specification. It keeps
snapshot-core and named-adapter costs separate, computes its oracle directly
from the source `f32` vectors in NumPy `f64`, and writes raw runs below
`target/lancedb-results/` by default. Raw vectors, downloaded data, and full
run output are therefore outside Git.

The Nix-first smoke command is:

```sh
nix develop -c uv run --frozen --project benchmarks/lancedb \
  python benchmarks/lancedb/harness.py \
  --workload benchmarks/lancedb/workloads/smoke.json
```

The same command with `standard.json` runs the explicit 1,000/10,000/100,000
point by 100/384/768 dimension matrix. `real-glove-25.json` verifies and uses
the pinned ANN-Benchmarks GloVe-25-angular download at 100,000 points;
`real-smoke.json` runs its 10,000-point evidence tier. Large profiles are never
part of `nix flake check` and must be requested explicitly.

Each repetition starts fresh runner processes and databases. Setup, immutable
snapshot build, named-adapter build, and LanceDB IVF-flat index build are
reported separately in raw results. Each runner performs one unmeasured query
in each mode before recording warm-process query samples. Declared concurrency
levels run as separate throughput batches and are not mixed with single-query
latency. `/usr/bin/time` records peak process RSS for every engine repetition.
The smoke profile exercises concurrency 1/4 for correctness and harness
iteration; it is not sufficient for broad performance claims.

Payloads contain `selectivity_bucket = uint64_id modulo 1000`, giving exact
50%, 10%, 1%, and 0.1% filters whenever the point count is a multiple of 1,000.
Mutation batches deterministically alter the first vector component by 0.001.
The retained-root profiler also provides `mutate-sample-stable`, which chooses
replacement IDs outside the bounded 8,192-point training sample. This isolates
the incremental codebook-reuse path without changing the standard workload or
silently relabeling its usually sample-changing mutation batches.
The workload schema is version 1 and is independent of the database's persisted
format version. The same harness can therefore compare format revisions without
changing its dataset, queries, mutations, or correctness oracle.

LanceDB uses an IVF-flat index with `round(sqrt(point_count))` partitions,
clamped to 1 through 256, and 8 probes. `git-vdb` uses the complete index
configuration stored in its root. These are declared operating points, not a
claim that their index structures are equivalent. Exact searches from both
engines are checked against the independent oracle; approximate recall and
filtered result count are then checked against that same oracle.
