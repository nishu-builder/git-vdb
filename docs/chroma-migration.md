# Migrating from Chroma

`git-vdb` keeps the familiar database, collection, upsert, get, query, count,
and delete concepts while using typed Rust rows and Git-backed persistence.

| Chroma concept | `git-vdb` equivalent |
|---|---|
| `PersistentClient(path=...)` | `git_vdb::open(path)` |
| `get_or_create_collection(name)` | `store.collection(name)`; first upsert creates it |
| `collection.upsert(...)` | `CollectionHandle::upsert` or `TextCollection::upsert_documents` |
| `query_texts=[...]` | `TextCollection::query_batch` |
| `query_embeddings=[...]` | `CollectionHandle::query_batch` |
| `where={...}` | `Filter` and `Condition` builders or JSON filters |
| `where_document={...}` | `Condition::document_contains` / `document_regex` |
| `include=[...]` | `Query::with_payload` / `with_vector` |

Chroma commonly accepts column-oriented arrays. `git-vdb` deliberately accepts
row-oriented `Point` and `Document` values so IDs, vectors, text, and metadata
cannot become misaligned. Export existing data as JSON Lines with one object per
point and import it in bounded batches:

```json
{"id":"doc-1","vector":[0.1,0.2],"payload":{"document":"hello","source":"guide"}}
```

```sh
git-vdb --db vectors.git upsert docs export.jsonl --batch-size 1000 --progress
```

Collection history, restore, validation, push, and pull are additional
Git-native operations rather than Chroma compatibility shims.
