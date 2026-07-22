# Performance climb after the 100,000-point LanceDB baseline

## Invocation

From the repository root, paste this into Codex:

```text
/goal Read docs/goals/lancedb-performance.md completely and execute it as the objective and completion criteria. Preserve format version 1, correctness, and unrelated work. Keep climbing through evidence-backed rungs until a documented stop condition is met. Do not push unless I explicitly authorize it.
```

## Objective

Reduce the measured performance gap between `git-vdb` and pinned LanceDB 0.34.0
without trading away `git-vdb`'s deterministic roots, Git interoperability,
historical reads, stale-writer protection, or exact search semantics.

Work in two deliberate tracks:

1. Exhaust the remaining read-path improvements that can preserve every byte and
   semantic of format version 1.
2. Design and, when the design is sufficiently specified and safe to isolate,
   prototype a deterministic format version 2 that attacks the structural build,
   mutation, storage, memory, and ANN costs that version 1 cannot remove.

LanceDB remains a comparator, not the correctness oracle. Exact brute-force
cosine search remains the independent query oracle. Do not optimize toward one
dataset by weakening general semantics or hiding unfavorable operating points.

## Accepted starting evidence

Use the existing harness and the accepted five-repetition 100,000-point
GloVe-25-angular run as the fixed starting baseline:

- report: `docs/benchmarks/lancedb-climb.md`;
- workload: `benchmarks/lancedb/workloads/real-glove-25.json`;
- raw result: `target/lancedb-results/real-glove-25-angular-20260722T013255Z`;
- summary SHA-256:
  `cbd20eb1837689becfbc5f5c6ba2d2a8a5aea0698571104b6d7a531f2ac69a82`;
- benchmark revision: `20776c52bd24fc7e3c400c94be415ec1d520297f`;
- machine class: EC2 `m6i.2xlarge`, 8 logical CPUs, 30.8 GiB RAM,
  x86_64 Linux.

The accepted median comparison is:

| Metric | git-vdb snapshot core | LanceDB 0.34.0 | Approximate gap |
|---|---:|---:|---:|
| Build | 53.445 s | 2.560 s | 20.9x slower |
| Exact query | 26.768 ms | 7.959 ms | 3.4x slower |
| Approximate query | 211.717 ms | 1.939 ms | 109.2x slower |
| Exact throughput, concurrency 4 | 157.0 qps | 264.3 qps | 1.7x lower |
| Approximate throughput, concurrency 4 | 19.39 qps | 891.0 qps | 46.0x lower |
| Peak RSS | 2.550 GB | 0.957 GB | 2.7x higher |
| On-disk bytes/point | 1,186.1 | 206.1 | 5.8x higher |
| 1% upsert | 4.144 s | 7.255 ms | 571x slower |
| 1% delete | 3.944 s | 2.750 ms | 1,434x slower |

At this baseline, both exact engines and the named adapter agree with the
independent oracle at k=1/10/100 for unfiltered and all filtered searches.
Maximum exact score error is `2.98e-8` for `git-vdb`. Approximate recall is
competitive at k=1/10, but both engines underfill some low-selectivity filtered
queries. Preserve that negative evidence.

## Non-negotiable gates

- Keep format version 1 readable and writable with identical canonical bytes,
  object IDs, root meaning, scoring, and tie order. A version-1 optimization may
  add ephemeral caches or change Git's physical loose/packed representation, but
  it must not change persisted logical objects.
- Equivalent data and configuration must retain history- and insertion-order-
  independent roots on every supported platform.
- Historical reads remain side-effect free. Ref updates retain compare-and-swap
  behavior. Failed writes cannot advance a collection head.
- Version-1 exact results must remain identical at k=1/10/100 for unfiltered and
  every declared filter selectivity. If arithmetic is refactored, add focused
  near-tie and zero-vector tests and require the existing score tolerance.
- Version-1 approximate acceleration must return the same ordered IDs and scores
  for the same root, query, and index parameters. Recall or result-count loss is
  not an acceptable speedup.
- Do not weaken or remove tests, workloads, repetitions, correctness gates, or
  resource measurements. Do not accept cherry-picked runs.
- Treat a 5% regression in any important non-target metric as material unless
  measured variance is larger or the tradeoff is explicitly documented and
  approved.
- Preserve unrelated changes and untracked files. Do not commit raw benchmark
  output, downloaded datasets, credentials, or generated repositories.
- Make focused local commits for accepted rungs. Do not push without explicit
  authorization.

## Track A: remaining format-version-1 opportunities

### Rung A1: attributable observability

Before changing another production path, add or run enough instrumentation to
attribute the remaining costs rather than infer them from whole-process time:

- phase-specific peak or retained memory for build, exact cache, approximate
  query, and mutation;
- Git ODB object and byte reads/writes for exact, approximate, build, and apply;
- candidate discovery versus candidate point decoding/scoring time;
- loose-object count/size, packed size, and clone/fetch transfer size;
- CPU profiles for the 100,000-point exact and approximate queries.

Prefer measurements outside production code where possible. If instrumentation
must enter production code, keep it optional and test that disabled behavior is
unchanged. Record commands, revisions, raw-result locations, and findings in a
tracked continuation report.

### Rung A2: exact top-k work reduction

The current cached exact path constructs a scored result for every eligible
point, clones every candidate ID, fully sorts the collection, and only then
truncates to k. Test a bounded top-k selection whose cost and retained result
state are proportional to k.

Materialize payloads and returned vectors only for final winners. Preserve the
existing descending-score and canonical-ID tie order exactly. Compare full
ordered results against the unchanged implementation on synthetic ties,
zero-vectors, every ID type, all filters, and the real workload.

Separately measure hoisting the query norm and retaining each cached point's
norm. Accept norm caching only if scores and ordering satisfy the exact gates;
do not combine it with top-k selection until the top-k result is independently
measured.

### Rung A3: compact exact-search representation

Profile the pointer-heavy `Vec<Point>` cache after A2. If memory access remains a
material exact bottleneck, evaluate a root-keyed read-only search view with
contiguous vector storage, compact IDs, cached norms, and payload references.
Keep public return values owned and unchanged. Measure cold cache construction,
warm latency, concurrency 1/4 throughput, and RSS together.

SIMD or per-query parallelism is eligible only after the representation is
measured. Preserve deterministic f64 accumulation and tie behavior unless an
explicit semantic proposal demonstrates why a change is safe. Report latency
and throughput separately because internal parallelism can improve one while
hurting the other.

### Rung A4: reuse the search view for approximate queries

The version-1 approximate path currently traverses Git bucket trees and decodes
candidate point objects even when the immutable root is warm. Test incremental
steps rather than one opaque cache:

1. resolve bucket candidates through a compact root-key-to-point lookup in the
   existing search view;
2. avoid repeat point-tree/vector/payload decoding for warm candidates;
3. if ODB bucket traversal remains dominant, build compact ephemeral postings
   keyed by table and signature for the resolved root.

The ephemeral view must be derived solely from the root and its metadata,
invalidated on root change, bounded in lifetime, safe across collection-handle
clones, and invisible to stored format bytes. Measure construction time and RSS,
not only warm query latency. Approximate IDs and scores must remain identical to
the unchanged version-1 implementation at every declared operating point.

After the same-result path is optimized, adaptive probing or exact fallback for
filtered underfill may be evaluated only as a separately named behavior change.
Do not disguise a new result-count contract as a performance patch.

### Rung A5: loose versus packed operation

Measure explicit repository maintenance without changing logical roots:

- loose and packed bytes per point;
- object counts and pack/index sizes;
- warm and cold exact/approximate query time;
- incremental mutation time before and after packing;
- full clone and one-snapshot fetch transfer bytes and time;
- historical-root readability and validation after packing.

If automatic packing is proposed, specify whether it is synchronous, explicit,
or background work. Keep maintenance time out of foreground timings only when it
is reported separately. Do not claim that packing improves initial build time
unless the measured build path actually writes packs directly.

### Rung A6: remaining version-1 writes

Use the ODB counters and profiles to test only narrow, evidence-backed changes
such as cached object-existence checks, parallel signature computation, or
batched object writes. The current apply path already reads touched IDs and
rewrites changed canonical paths; do not repeat broad incremental-update work.

Stop optimizing version-1 writes when the dominant cost is required canonical
Git object creation or validation. Record that floor honestly rather than
adding complexity for sub-noise gains.

## Track B: deterministic format version 2

Begin the design once Track A evidence confirms which costs are structural.
Adding version 2 must not change the meaning or default handling of existing
version-1 roots.

### Required proposal

Write a reviewable design document before production implementation. It must
specify:

- a canonical Git tree/object layout with substantially fewer objects and bytes;
- deterministic, history-independent vector sharding and IVF-flat construction;
- deterministic centroid training or selection, assignment, posting order, and
  tie rules across platforms and insertion histories;
- contiguous or chunked vector encoding, payload/ID lookup, and filter strategy;
- exact search, ANN search, mutation, validation, diff, materialization, clone,
  fetch, and garbage-collection behavior;
- chunk rewrite and structural-sharing tradeoffs for 1%/10%/100% mutation;
- explicit numeric representation and accumulation rules;
- compatibility and migration: version detection, v1 coexistence, opt-in v2
  creation, and whether conversion creates a new root/history boundary;
- corruption detection, resource bounds, failure atomicity, and stale-writer
  behavior;
- benchmark hypotheses, expected tradeoffs, and rejected alternatives such as
  history-dependent training or nondeterministic graph construction.

IVF-flat is the leading hypothesis, not a predetermined conclusion. Reject it
if a measured deterministic prototype shows that another layout better
preserves Git-native semantics and the required performance profile.

### Prototype gate

Prototype behind an explicit experimental API, feature, or standalone benchmark
path that cannot silently write version 2 through the stable version-1 default.
Prove first on smoke and 10,000-point workloads, then run the unchanged pinned
100,000-point protocol on the same machine class.

At minimum, compare:

- exact agreement and ANN recall/result count;
- build, exact, approximate, and mutation latency distributions;
- concurrency 1/4 throughput;
- phase-specific and process peak RSS;
- logical bytes, packed bytes, objects per point, and bytes per point;
- clean-build root equality across insertion orders and mutation histories;
- incremental versus clean-rebuild root equality;
- clone/fetch transfer behavior and historical-root reads.

Do not promise LanceDB parity as a completion criterion. A format-2 prototype is
successful only if it produces material gains in several structural categories
without an unexplained correctness, determinism, operational, or resource
regression.

## Climbing and acceptance protocol

For every production rung:

1. Preserve an unchanged baseline executable or revision.
2. Write the evidence, bottleneck, hypothesis, expected gain, and tradeoff before
   editing production code.
3. Change one primary variable and add focused correctness tests.
4. Run formatting, Clippy, the full test suite, documentation checks, and the
   differential smoke workload.
5. Use interleaved baseline/candidate samples on the same machine. Use enough
   repetitions to establish noise; retain p50/p95/p99 and concurrency results.
6. Graduate accepted changes to the pinned real workload. Rerun the full
   100,000-point protocol for changes that can affect the reported frontier.
7. Accept only when correctness is green, the target movement exceeds noise,
   and important regressions remain below the materiality threshold or are an
   explicitly accepted Pareto tradeoff.
8. Revert only the unsuccessful rung's own production edits. Preserve concise
   negative evidence so the same idea is not retried without new information.
9. Update the tracked report and make one focused local commit for each accepted
   rung. Never push without explicit user authorization.

## Completion and stop conditions

Continue autonomously until all applicable Track A rungs have measured outcomes
and one of the following is documented:

- the remaining version-1 query opportunities are accepted or rejected, packed
  behavior is characterized, and the dominant write/storage floor is shown to
  be structural;
- three consecutive well-formed attempts at the active frontier fail to improve
  the target beyond noise without a correctness or material-regression cost;
- further progress requires product semantics, a migration policy, a format-2
  design decision, unavailable hardware/data, or external authority;
- a format-2 proposal and prototype have enough evidence for a user review
  before committing to production compatibility and migration obligations.

The final report must include accepted and rejected hypotheses, before/after
metrics, correctness evidence, memory and storage tradeoffs, local commits,
reproduction commands, remaining uncertainty, an honest comparison with
LanceDB, and the single highest-leverage next decision. Do not call the goal
complete merely because one metric improves or a proposal exists without the
evidence required by its current gate.
