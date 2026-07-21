# Continuous correctness and performance climb against LanceDB

## Invocation

From the repository root, paste this into Codex:

```text
/goal Read docs/goals/lancedb-climb.md completely and execute it as the objective and completion criteria. Keep climbing through evidence-backed correctness and performance rungs until a documented stop condition is met. Preserve format version 1 and unrelated work. Do not push unless I explicitly authorize it.
```

## Objective

Make `git-vdb` measurably more correct, efficient, and understandable through a
reproducible differential harness against one pinned version of LanceDB. Work in
successive evidence-driven rungs: establish a trustworthy baseline, identify one
measured bottleneck, make the smallest sound improvement, verify it, and retain
it only when the evidence supports it.

LanceDB is a comparator, not a specification and not an opponent that must be
beaten everywhere. Compare overlapping embedded-database behavior fairly. Treat
`git-vdb`'s deterministic roots, Git interoperability, and snapshot/ref split as
additional invariants with their own measurements.

## Non-negotiable correctness gates

- Preserve all documented CLI, `SnapshotEngine`, `Snapshot`, `Database`, and
  `Collection` semantics unless a change is explicitly justified and approved.
- Preserve format-version-1 bytes, object IDs, root meaning, scoring, and
  canonical tie order. If an improvement requires a format change, write a
  version-2 proposal instead of silently changing version 1.
- Equivalent point sets and configuration must produce the same root across
  insertion order, mutation history, repository layout, process, and supported
  platform.
- Exact cosine search is the correctness oracle. Approximate performance never
  counts as an improvement when recall, filtering correctness, or result count
  falls outside the declared acceptance threshold.
- Historical roots remain readable; reads remain side-effect free; stale writers
  cannot replace a collection head; failed writes cannot advance a ref.
- Never weaken, delete, skip, or loosen a test or workload merely to improve a
  metric. Do not hide failures, cherry-pick favorable runs, or change a workload
  after seeing a result without invalidating and rerunning its baseline.
- Preserve unrelated user changes and untracked files. Do not commit generated
  databases, downloaded datasets, credentials, or bulky raw benchmark output.

## Fair comparison contract

Pin the exact LanceDB version and all harness dependencies. Record the lock,
`git-vdb` revision, OS, architecture, CPU, memory, Rust toolchain, benchmark
configuration, dataset checksum, and whether each run is cold or warm.

Use the same source vectors, comparable ID domain, cosine metric, payloads,
filters, mutation batches, query vectors, `k`, process concurrency, and machine
for both engines. Keep process/setup time separate from build, mutation, and
query time. Separate `git-vdb`'s immutable snapshot core from its commit/ref
adapter so their costs are visible rather than blended.

Where semantics differ, compare only the common subset and report the
difference. Do not emulate a missing feature in a way that makes one engine look
artificially slow. Git-only behavior is measured independently, not assigned a
fake LanceDB equivalent.

## Harness to establish first

Create a maintainable benchmark area with:

1. A versioned workload schema describing build, upsert, delete, exact query,
   approximate query, filter, snapshot selection, and historical read steps.
2. `git-vdb` snapshot-core and named-adapter runners plus one LanceDB runner.
3. A framework-independent exact cosine oracle for differential result checks.
4. Deterministic synthetic uniform and clustered datasets, followed by at least
   one pinned real angular/cosine dataset with checksum and license/source notes.
5. Machine-readable raw results outside Git and a compact tracked summary of
   methodology, accepted results, negative results, and remaining uncertainty.
6. One Nix-first command that builds/runs the harness and a cheap smoke profile
   suitable for CI. Large performance runs must remain explicit, not burden CI.

Start at a size that makes harness iteration cheap, then graduate without
changing the protocol: 1,000, 10,000, and 100,000 points; dimensions 100, 384,
and 768 where supported; `k` values 1, 10, and 100. Add a million-point tier only
after the 100,000-point methodology is stable.

## Required measurements

For each relevant workload, capture:

- exact result agreement and score tolerance;
- approximate recall@1, recall@10, and recall@100 against the exact oracle;
- filtered recall and result count at approximately 50%, 10%, 1%, and 0.1%
  selectivity;
- build, 1%/10%/100% upsert, delete, and query p50/p95/p99 wall time;
- throughput at declared concurrency, without mixing it with single-query
  latency;
- peak RSS, on-disk bytes, and bytes per point;
- for `git-vdb`, vectors scored, Git objects and bytes read/written, loose and
  packed size, structural reuse, and clone/fetch transfer size;
- cold-process and warm-process behavior, with warmup excluded from samples.

Use enough repetitions to quantify noise. Prefer medians and percentile
distributions over a single total. A performance claim is valid only when it
exceeds the observed noise floor and its exact workload can be reproduced.

## Climbing loop

Repeat this loop autonomously while a safe, measurable rung remains:

1. Run correctness gates and the unchanged baseline protocol.
2. Profile or instrument before selecting a bottleneck. Write down the metric,
   evidence, hypothesis, and expected tradeoff before editing production code.
3. Change one primary variable with the smallest maintainable patch.
4. Add or strengthen focused correctness tests when behavior could regress.
5. Run `nix flake check`, differential checks, and the same benchmark samples.
6. Accept the rung only if correctness is green, the target improvement exceeds
   measurement noise, and no important metric regresses materially. Treat a 5%
   regression as material unless the recorded variance is larger or a clear
   Pareto tradeoff was explicitly approved.
7. Revert only the unsuccessful rung's own edits when evidence rejects it.
   Preserve the measurements and concise negative finding so it is not retried
   without new evidence.
8. Record accepted before/after results, commands, revision, and explanation.
   Make a focused local commit for an accepted rung. Do not push unless the
   initiating user explicitly authorizes it.

Prefer work that attacks known costs without changing the public contract:
incremental `apply` proportional to changed points, tree/object reuse, batched
or cached object decoding, exact-scan efficiency, packed-repository behavior,
and observability. Do not introduce a new ANN format until real-dataset evidence
shows the version-1 LSH frontier and a versioned design is reviewed.

## Stop conditions and final report

Continue until the harness and baseline are reproducible, correctness gates pass,
and either:

- the current measured bottleneck has been improved and no remaining safe
  optimization is supported by the profiles; or
- three consecutive well-formed attempts at the current frontier fail to produce
  an improvement beyond noise without a correctness or material-regression cost;
  or
- further progress requires a format-version change, new product semantics,
  unavailable hardware/data, external authority, or a user decision.

Do not declare victory merely because `git-vdb` wins one chart, and do not call
the goal blocked merely because an experiment is slow or negative. At the end,
report the comparison honestly: where `git-vdb` wins, loses, or offers different
semantics; every accepted commit; exact reproduction commands; failed
hypotheses; remaining bottlenecks; and the single highest-leverage next rung.
