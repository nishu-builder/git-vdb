# Format-version-2 standalone prototype

This directory evaluates the design candidate in
`docs/format-v2-ivf-flat.md`. It is not linked into the stable `git-vdb` API and
cannot change the default format-version-1 writer.

Run it with an existing harness `run.json` whose vector paths are valid:

```sh
nix develop -c uv run --frozen --project benchmarks/lancedb \
  python benchmarks/format2/prototype.py \
  --run-spec target/lancedb-results/RESULT/CASE/run.json \
  --output target/lancedb-results/RESULT/CASE/format2-prototype.json
```

The prototype writes candidate point shards, centroids, and postings into a
temporary bare Git repository, then reports the real Git tree ID, logical blob
bytes, loose repository bytes, object count, reversed-input root equality,
query timings, ANN recall, and filtered result counts.

The prototype also exercises concurrency, Git packing and transfer, and a
changed-shard mutation serializer whose root must equal a clean serialization.
Its report records remaining gates, including larger workloads, phase RSS, and
fully incremental centroid/assignment maintenance. Its numbers are not
production format-v2 claims until those gates are implemented and pass.
