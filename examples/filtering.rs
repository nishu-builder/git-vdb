use git_vdb::{open, Condition, Filter, Point, Query};
use serde_json::json;

fn main() -> git_vdb::Result<()> {
    let temporary = tempfile::TempDir::new()?;
    let docs = open(temporary.path().join("vectors.git"))?.collection("docs");
    docs.upsert([
        Point::new("guide", [1.0, 0.0]).with_metadata(json!({"kind": "docs"}))?,
        Point::new("code", [0.9, 0.1]).with_metadata(json!({"kind": "source"}))?,
    ])?;
    let result = docs.advanced()?.query(
        Query::new([1.0, 0.0], 5)
            .with_filter(Filter::must([Condition::matches("kind", "docs")]))
            .with_payload(),
    )?;
    assert_eq!(result.points[0].id.to_string(), "guide");
    Ok(())
}
