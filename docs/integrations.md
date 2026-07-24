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
