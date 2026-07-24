# git-vdb Python client

This package exposes a small Chroma-shaped Python API over the `git-vdb` JSON
CLI. Install a FastEmbed-enabled `git-vdb` binary, ensure it is on `PATH`, then:

```python
from git_vdb import PersistentClient

collection = PersistentClient("./vectors.git").get_or_create_collection("docs")
collection.upsert(ids=["east"], embeddings=[[1.0, 0.0]])
print(collection.query(query_embeddings=[[0.9, 0.1]], n_results=1))
```

Set `GIT_VDB_BIN` to use a binary outside `PATH`. Storage, filtering, history,
and concurrency semantics remain implemented by the Rust crate rather than
being duplicated in Python.
