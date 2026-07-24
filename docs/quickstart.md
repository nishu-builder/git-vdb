# Five-minute quickstart

Create a Rust project and add the crate:

```sh
cargo new vector-search
cd vector-search
cargo add git-vdb
```

Open a database, write two points, and search:

```rust,no_run
use git_vdb::{open, Point};

fn main() -> git_vdb::Result<()> {
    let db = open("./vectors.git")?;
    let docs = db.collection("docs");
    docs.upsert([
        Point::new("east", [1.0, 0.0]),
        Point::new("north", [0.0, 1.0]),
    ])?;
    let hits = docs.search([0.9, 0.1], 1)?;
    assert_eq!(hits[0].id.to_string(), "east");
    Ok(())
}
```

`open` creates a bare repository when the path is missing. The first upsert
creates the collection and establishes its vector dimension. Later calls to
`open` reuse the same data.

For the CLI equivalent, install the binary and run:

```sh
cargo install git-vdb
git-vdb --db vectors.git upsert docs --id east --vector '1,0'
git-vdb --db vectors.git search docs --vector '0.9,0.1'
```

Continue with [persistence](persistence.md), [filtering](filtering.md), or
[history and transport](history.md).
