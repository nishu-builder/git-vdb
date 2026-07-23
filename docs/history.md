# History and Git transport

Every named write creates a commit and advances
`refs/git-vdb/collections/<name>` atomically. Ordinary searches do not create
objects or move refs.

```rust
use git_vdb::{open, Point};

# fn main() -> git_vdb::Result<()> {
# let temporary = tempfile::TempDir::new()?;
let docs = open(temporary.path().join("vectors.git"))?.collection("docs");
docs.upsert([Point::new("v1", [1.0, 0.0])])?;
docs.upsert([Point::new("v2", [0.0, 1.0])])?;
let history = docs.advanced()?.history(10)?;
assert_eq!(history.len(), 3); // create plus two writes
# Ok(())
# }
```

Push or fetch the collection ref with stock Git:

```sh
git --git-dir vectors.git push origin \
  refs/git-vdb/collections/docs:refs/git-vdb/collections/docs
```

Use `Collection::at` for historical reads, `diff` to compare roots, and
`validate` before accepting data from an untrusted transport.
