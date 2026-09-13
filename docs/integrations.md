# Framework integrations

The core integration boundary is intentionally small: frameworks can store
`Document` values, run `TextQuery` values, or use vectors directly through
`CollectionHandle`. An adapter only needs to map its document ID, text, metadata,
and embedding provider onto those types.

For LangChain or LlamaIndex today, use their Rust document/chunking layer with a
small application adapter that calls `upsert_documents` and `query_batch`. Keep
the provider's model and revision in `Embedder::model_id`; `git-vdb` persists and
checks that identity on every reopen.

For language-agnostic pipelines, use the JSON CLI:

```sh
producer | git-vdb --db vectors.git upsert docs - --batch-size 1000
git-vdb --db vectors.git search docs --vector '[0.1,0.2]' --format json
```

The CLI writes result data only to stdout and progress or errors to stderr, so it
is safe to compose with subprocess-based tools.

## Caos semantic source search

The optional [Caos adapter](../integrations/caos/README.md) provides
`caos-tools/semantic-search` for natural-language queries over immutable source
trees and historical commits. Results carry exact text, paths, line spans,
source tree/commit IDs and file blob IDs. A pinned CPU MiniLM model runs from
packaged assets, and unchanged file content reuses embedding jobs across edits,
renames and queries.

The adapter returns its index as a retained subtree and queries through the
format-2 `SnapshotReader` object interface. It has independent dependencies and
Nix packaging; the default core build needs neither Caos nor a model download.
See the [measured implementation report](../integrations/caos/REPORT.md) for
correctness, cache traces, cold/warm costs and limitations.

For provider-independent immutable text workflows, use
`SnapshotEngine::build_text`, `Snapshot::with_embedder`, and `TextSnapshot` as
shown in [the embedding guide](embeddings.md#immutable-text-snapshots).
