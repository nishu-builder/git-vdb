# Persistence and reopening

`Store::open` accepts bare and ordinary Git repositories. It creates a bare
repository only when the path is missing or an existing directory is empty. A
nonempty directory that is not a repository is rejected without changing it.

```rust
use git_vdb::{open, Point};

# fn main() -> git_vdb::Result<()> {
# let temporary = tempfile::TempDir::new()?;
let path = temporary.path().join("vectors.git");
open(&path)?.collection("docs").upsert([
    Point::new("stable-id", [1.0, 0.0]),
])?;

let reopened = open(&path)?.collection("docs");
assert_eq!(reopened.count()?, 1);
# Ok(())
# }
```

Collection handles are inexpensive and can be cloned. Reads never create a
missing collection. The first nonempty upsert creates it and infers its
dimension; explicitly configured empty collections remain available through
`Database::create_collection`.
