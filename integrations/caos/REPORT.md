# Caos integration: implementation evidence

Validated 2026-09-13 on x86_64 Linux. This is a source-search integration;
selected conversation memory remains separate follow-up work.

## Revisions and environment

- Measured worker/core revision: `bad622fae3447f049adbfbcb86aa1335119125e8`.
  Subsequent changes add documentation and measurement helpers; the final
  publication also runs a fresh integration smoke test.
- Caos and realistic source corpus: `8e44b8f51d97155c2287f47e6ae57eb4d4a23e20`;
  source tree `67fe4d4d391d57ecaac07c4023621f45d6f6bc79`.
- Rust 1.97.1; Nix inputs are fixed by the repository and adapter `flake.lock`.
  Core formats 1 and 2 and existing Rust/CLI/Python behavior remain compatible.
- Measured semantic-worker image digest:
  `sha256:19cb422505eaefbbb1a98a90212605e444f0ebf82dd6f9015c7afc04618bb721`.
  The isolated stack used the pinned Caos binaries and standard worker images.
- Intel Xeon Platinum 8375C at 2.90 GHz, 8 vCPU / 4 physical cores, x86_64,
  approximately 30 GiB RAM, no swap. Two Caos worker slots shared two CPU cores;
  each worker had a 14 GiB memory ceiling. The stack had two CPUs. Reader
  benchmarks were pinned to CPU 6, with no filesystem-cache eviction.
- Model: Xenova/all-MiniLM-L6-v2, revision
  `751bff37182d3f1213fa05d7196b954e230abad9`, Apache-2.0, 384 dimensions.
  Full artifact checksums are in [model.lock.json](model.lock.json).
  Python 3.14.6, NumPy 2.5.0, ONNX Runtime 1.26.0, tokenizers 0.22.2;
  one-thread sequential CPU inference, masked mean pooling, L2 normalization,
  float32 output, 256-token truncation, no query/document prefix.
- Embedding identity:
  `minilm/c00131d8468a8ef967a391fb4f9d19d6cfb3a947f38851b061bdea21087eb4e9`.
  Adapter SHA-256:
  `025ab0fc3fe499f804105660bf9660c7e9a3aae3c4c42e1ff56962dc01edb54d`.

A clean stack build needs substantial toolchain/image storage; allow at least
25 GiB free. The measured host had limited disk space, so this task isolated
its Nix store and build/model caches in RAM. Reader numbers therefore describe
cached local files, not cold physical storage or remote WAN latency.

## Correctness and actual reuse

Rust tests compare both readers across exact/approximate queries, filters,
probe/candidate budgets, zero vectors, near ties, empty snapshots and visited
corruption. Text-snapshot tests cover creation, reopen, historical queries,
mutation, batching and model rejection before inference. Adapter tests check
UTF-8 chunk boundaries, duplicates, rename/delete provenance and clean-build
root equality. Neither snapshot API creates commits or refs.

The real `smoke.py` suite uses the packaged model through Caos, including five
salted fresh executions. Ordinary calls omit the salt. Computation records,
including continuation jobs inside existing containers, establish:

| Change | Fresh document jobs | Fresh query jobs | Observation |
|---|---:|---:|---|
| Repeat or equivalent agent dispatch | 0 | 0 | Same result identity |
| New query | 0 | 1 | Reuses index/document work |
| One-file edit | 1 | 0 | Other contents reused |
| Rename | 0 | 0 | New path; old path absent |
| Deletion | 0 | 0 | Deleted occurrence absent |
| Historical source | 0 | 0 | Original result identity |
| Chunk limit 1200 to 64 | 4 | 0 | Document job keys change |
| Model token limit 256 to 128 | 4 | 1 | New embedding identity |

The fixture has four distinct eligible blobs, including an empty file which
creates a job but performs no inference; two identical nonempty files retain
separate source occurrences. Each forced-fresh run executed all four document
jobs and one query job. Source blob IDs and exact inclusive line ranges were
verified against Git. The source and model fixtures were restored afterward.

Actual edge runs returned canonical zero-point indexes for an empty file and a
binary-only corpus. Invalid scope, zero limit and empty query returned readable
`FAILED` result trees with exit status 1. Successful source containing that
reserved marker is rendered as `Failed` only in the human report; structured
text is exact.

Five provider processes inside a network-disabled container produced identical
vector JSON SHA-256
`6da66733c33f9d02c96c9f570459070bcab25cf6d95efc4cab5f3e5299f53d0c`.
The retry query scored 0.47546 against retry text and 0.03430 against cake text.
This checks a real semantic model, not just dimensions or a fixture embedder.
Byte reproducibility is established for this tested runtime/CPU, not every
CPU generation or future provider.

## Fixed workloads and reader comparison

The synthetic corpus has 4,096 deterministic 32-dimensional points with roughly
1 KiB payloads. The realistic corpus is the entire pinned Caos tree, selected
by the documented allowlist/exclusions before comparing readers: 334 files,
319 distinct blobs, 2,934,055 source bytes, 3,233 passages, chunk limit 1200
characters or 24 lines. Two symlinks and one oversized source file are recorded
as skipped; extensions and excluded directories are filtered before reading.

The fixed natural-language query is “How does Caos reuse unchanged work after
a source file edit?” All workloads request ten hits with payloads:

- selective: one probe, 32 scored candidates;
- broad: 64 probes, candidate limit 4096;
- exact: full scoring;
- filtered: broad parameters, `path == "README.md"` for Caos, `group == 1`
  for the synthetic fixture.

Each reader/workload has five fresh processes, alternating reader order, with
five additional warm queries per process. Fresh latency includes opening the
reader and its first query. Eager opening imports the whole canonical directory
into Git and uses the existing cached `Snapshot`; lazy opening uses
`DirectorySource`. Both readers returned exactly the same ordered results and
query counters. Exact results also matched a separate Python float64 cosine
oracle over float32 inputs. Recall is measured against that oracle.

Values below are median milliseconds [minimum–maximum]. Logical bytes count
complete blob bytes for the first query, excluding tree bytes and transport.
RSS is the median Linux peak resident set for each process in KiB.

### Pinned Caos corpus

| Workload / reader | Fresh ms [min–max] | Warm ms [min–max] | Blob bytes | Peak RSS KiB | Recall@10 |
|---|---:|---:|---:|---:|---:|
| selective / eager | 373.27 [372.76–376.27] | 0.13 [0.13–0.23] | 9,148,604 | 17,012 | 0.5 |
| selective / lazy | 5.47 [4.65–5.64] | 3.69 [3.62–3.96] | 2,888,549 | 7,748 | 0.5 |
| broad / eager | 380.66 [379.78–380.83] | 4.96 [4.49–5.61] | 9,148,604 | 24,152 | 1.0 |
| broad / lazy | 11.30 [10.18–11.43] | 8.19 [8.07–8.67] | 5,756,882 | 10,904 | 1.0 |
| exact / eager | 367.99 [367.39–368.24] | 1.59 [1.57–1.79] | 9,148,604 | 16,408 | 1.0 |
| exact / lazy | 10.69 [9.78–10.83] | 7.70 [7.60–8.04] | 5,638,396 | 11,144 | 1.0 |
| filtered / eager | 374.13 [373.97–374.20] | 0.45 [0.43–0.82] | 9,148,604 | 17,012 | 1.0 |
| filtered / lazy | 25.27 [24.59–25.89] | 21.32 [20.50–21.88] | 8,818,826 | 15,624 | 1.0 |

### Deterministic fixture

| Workload / reader | Fresh ms [min–max] | Warm ms [min–max] | Blob bytes | Peak RSS KiB | Recall@10 |
|---|---:|---:|---:|---:|---:|
| selective / eager | 103.04 [102.41–107.57] | 0.08 [0.08–0.16] | 5,877,942 | 14,192 | 0.4 |
| selective / lazy | 2.79 [2.50–2.84] | 2.14 [2.09–2.54] | 809,910 | 5,748 | 0.4 |
| broad / eager | 108.65 [108.57–111.01] | 3.21 [2.90–6.09] | 5,877,942 | 23,596 | 1.0 |
| broad / lazy | 6.53 [6.37–6.98] | 5.37 [5.26–5.91] | 1,392,052 | 7,024 | 1.0 |
| exact / eager | 98.22 [97.95–98.72] | 0.28 [0.26–0.37] | 5,877,942 | 13,808 | 1.0 |
| exact / lazy | 6.19 [5.66–6.47] | 4.69 [4.59–4.95] | 1,358,500 | 7,016 | 1.0 |
| filtered / eager | 105.52 [104.68–105.56] | 1.52 [1.40–2.23] | 5,877,942 | 16,728 | 1.0 |
| filtered / lazy | 19.81 [18.61–19.95] | 17.17 [16.43–17.63] | 5,677,226 | 13,260 | 1.0 |

The warm regressions exceed both 5% and measured noise. They arise because the
selective reader drops decoded shards after each query, whereas `Snapshot`
retains them. The new reader is optional and used for fresh Caos invocations;
the existing local API keeps its cache. These results support selective reads
for fresh workers, not replacing the default cached reader everywhere.

On Caos, the selective query scored 32 vectors, discovered 33 candidates and
read 66 blobs; broad search scored all 3,233 vectors and read 202 blobs. The
README filter scored only 41 vectors but discovered all 3,233 candidates and
read 258 blobs, 96.4% of the index's blob bytes. Hash-distributed shards limit
savings: a single candidate can require a whole shard. Low recall at the small
budget is reported, not hidden by tuning the workload afterward. The tool's
current default probes all 64 centroids on this corpus.

## Real transport and stage costs

Five forced-fresh Caos search stages reused the same index and query vector.
Every visited index object was matched to a successful GET through an isolated
HTTP measurement proxy. Each run fetched 210 index objects (202 blobs and eight
trees), with **5,768,486 HTTP response-body bytes**; all GET bodies in the stage
totaled 5,804,544 bytes. The reader separately reported 5,756,882 logical blob
bytes. HTTP bodies include object framing but exclude headers/TCP; the eager
local benchmark is not an eager HTTP measurement.

All five runs produced result `b5cce4ac44fd1136032a9dfbfe00d196fbe9fcbb`
and index `919b2380a6116666cf138b70325c346999a8399b`.
Fresh search dispatch plus result materialization was 647.44 ms median
[644.49–866.92]. This includes model-identity verification, object transport,
Caos scheduling and client materialization. The stack and images were already
available; this is not a cold stack/image-download measurement.

Separately measured boundaries:

| Boundary | Measurement |
|---|---|
| Cached runtime-container create/start/exit/remove | Five runs: 191.43 ms median [190.06–229.79]; no image pull or Caos scheduling |
| Packaged provider process, three texts | Five runs: 497.20 ms median [464.17–515.48], peak child RSS 208,956 KiB; startup/imports/checks/tokenization/model init/inference/serialization included |
| Source read and blob verification | Five runs: 9.88 ms median [9.86–202.39]; first filesystem traversal was slower |
| Actual chunker, 334 selected files | Five runs: 5.49 ms median [5.46–5.63]; separate from inference |
| Whole realistic Caos pipeline | One forced-fresh run: 199.713 s, including selection, embedding, assembly and search |
| Realistic document jobs | 319 jobs, 650 ms median [444–20,632]; file size and passage count vary, so these are not equivalent repetitions |
| Realistic index assembly | 2,386 ms in that pipeline; separately recorded continuation |
| Realistic query embedding / search | 451 ms / 594 ms in that pipeline |

Caos parent-stage elapsed times include children and continuations and must not
be summed with child times. The five small-fixture traces below give repeated
stage boundaries; selection is the interval from index-stage start to the
first embedding request and includes manifest/model-identity work.

| Fixture stage | Samples | Median ms [min–max] |
|---|---:|---:|
| total | 5 | 1753.16 [1745.15–1787.89] |
| selection | 5 | 166.00 [165.00–180.00] |
| document jobs | 15 | 450.00 [444.00–636.00] |
| assembly | 5 | 14.00 [13.00–14.00] |
| query embedding | 5 | 433.00 [429.00–438.00] |
| search | 5 | 167.00 [167.00–168.00] |

## Reproduce

Start the pinned stack and configure the consumer remote using the
[integration guide](README.md). Use a clean disposable clone for the mutating
fixture suite, and keep output outside it:

```sh
python3 integrations/caos/smoke.py /tmp/caos-tools/bin/caos-cli   /tmp/vdb-smoke --runner-log /path/to/this-stack/runnerd.log
```

The realistic corpus command uses complete history because pinned Caos rejects
shallow pushes:

```sh
git fetch --no-tags https://github.com/Metta-AI/caos   8e44b8f51d97155c2287f47e6ae57eb4d4a23e20
caos-cli run-tool semantic-search   --in:commit=8e44b8f51d97155c2287f47e6ae57eb4d4a23e20   --query='How does Caos reuse unchanged work after a source file edit?'   --limit=10 --run-salt=choose-a-new-validation-salt
# Substitute the printed result ID:
caos-cli get RESULT_ID /tmp/vdb-caos-result
```

For actual transport, run `http_meter.py --listen WORKER_REACHABLE_ADDRESS
--port METER_PORT --upstream http://STACK_ADDRESS:STACK_PORT > /tmp/meter.jsonl`
and configure this isolated stack's runner to give workers that meter URL as
`CAOS_SERVER_URL`. Leave the stack otherwise idle during measurement. The meter
supports the pinned worker's non-chunked requests and counts HTTP bodies; it is
a development measurement helper, not a general-purpose proxy.

```sh
python3 integrations/caos/transport_benchmark.py /tmp/caos-tools/bin/caos-cli   RESULT_ID 'How does Caos reuse unchanged work after a source file edit?'   /tmp/vdb-transport --meter-log /tmp/meter.jsonl
cargo build --release --example snapshot_read_benchmark --locked
target/release/examples/snapshot_read_benchmark export /tmp/vdb-caos-result/snapshot
python3 integrations/caos/prepare_corpus_benchmark.py   /tmp/vdb-caos-result/snapshot /tmp/vdb-transport/query-vector.json
python3 integrations/caos/benchmark.py   target/release/examples/snapshot_read_benchmark   /tmp/vdb-caos-result/snapshot /tmp/vdb-corpus-benchmark
```

The export command performs full canonical validation. `profile_chunks.rs`
accepts the result manifest and a checkout at the pinned source commit.
`profile_model.py` accepts the actual packaged embedding command and an output
JSON path; run it inside a container with `--network none` to repeat the offline
model check. The image's `/embed --describe` reports the full effective identity.
The synthetic reproduction commands are in the integration guide.

Completed checks: `nix flake check` on Linux; default Rust all-target tests,
doctests and Clippy with warnings denied; adapter all-target tests and Clippy;
Python client/CLI compatibility; real Caos smoke, empty/error cases, offline
model runs, independent-oracle reader benchmarks and fresh HTTP measurements.
CI also checks optional adapter contracts without a server or model download.

## Artifact identities

Bulky raw outputs are retained outside Git. SHA-256 values below identify the
measured artifacts, not a promise that elapsed-time logs reproduce byte-for-byte.

| Artifact | SHA-256 |
|---|---|
| Synthetic points | `9894ad790d75fed41fd98bf4652f88a49e63a3f0a1c265c61baa384b05ea11f3` |
| Synthetic queries | `952eb2d9c65a2fe740fb83e5476e110190b7e4c30d8d44fd75b4cc039e18af30` |
| Synthetic raw benchmark | `2ecf78022bc459eaefaa2b5208889b25dee692141a850e8bd339c5205eb25685` |
| Caos points | `d190475e738b71652be7bbdcd18aff61699a170af863bdb2cac1a87eb2d75e20` |
| Caos queries | `47c60a0fa1c80904292c7fa2dc97540892d4c81399ece9a8b033506494aa6c59` |
| Caos raw benchmark | `42256f96dc5da00f9e12107be0719a66e2323ede98a9514c924c75e61c00552b` |
| Caos manifest | `e6de659f3e6e4f6d732beacb488c24f9d81a6a507489cfd08dd6ed39838a80a4` |
| Caos computation trace | `469075bf61d58c2fa94847bb04f381a9b20b9b1968a66140d38c9e100b159642` |
| Cache/provenance runs | `297cb87bd3f59fff96834ade7dd0124497206a0482775f0d7cdcd71f902265ef` |
| Transport summary | `b88451ff15a301b3db9b720a64d7ee415d37a74cd57cc714afed175eb0cdd9d2` |
| Offline model profile | `19f9090f2db3c9f7bfa6f661e6217a89ce0f5eba9fb9ec686f2beb240acb06a7` |
| Chunk profile | `96848fb5440db56f1ba3752d76390d6caa4ee1667e3e49f63bc4e4f5c51eea16` |
| Container startup | `63a3644d49cc11172726273f1c18fc142befe51c61f9232ee43c2184e628b64a` |
| Empty/error runs | `43eb3568187aee240c76f9bbbeab2850b1f8a0a1b0cd410dd62416cba363e085` |

Remaining limitations: the optional selective reader supports only format 2
and validates visited objects; callers must authenticate immutable roots and
use full snapshot validation when needed. Payload filters may require all
shards. Model inference is CPU-only and tested on this architecture. Source
selection is an explicit allowlist rather than Git ignore-file evaluation;
non-UTF-8 filenames fail the request, while invalid UTF-8 file contents are
skipped. Worker implementation/build-input changes conservatively version
computation keys. No storage-format change or conversation-memory scope is
introduced.
