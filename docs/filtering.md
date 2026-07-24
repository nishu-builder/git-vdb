# Filtering and detailed queries

The small `search` method returns IDs, scores, and payloads. Use the detailed
collection API when a search needs filters, vectors, roots, or execution stats:

```rust,no_run
use git_vdb::{open, Condition, Filter, Point, Query};
use serde_json::json;

fn main() -> git_vdb::Result<()> {
    let docs = open("./vectors.git")?.collection("docs");
    docs.upsert([
        Point::new("guide", [1.0, 0.0]).with_metadata(json!({"kind": "docs"}))?,
        Point::new("code", [0.9, 0.1]).with_metadata(json!({"kind": "source"}))?,
    ])?;
    let result = docs.query(
        Query::new([1.0, 0.0], 5)
            .with_filter(Filter::must([Condition::matches("kind", "docs")]))
            .with_payload(),
    )?;
    assert_eq!(result.points[0].id.to_string(), "guide");
    Ok(())
}
```

Filters support equality, numeric ranges, existence, inclusion/exclusion sets,
array containment, stored-document substring/regex matching, typed IDs, and
nested Boolean conditions. Detailed results also identify the immutable root and
report how much exact or approximate work was performed.
