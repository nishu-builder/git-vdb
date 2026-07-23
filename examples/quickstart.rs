use git_vdb::{open, Point};

fn main() -> git_vdb::Result<()> {
    let temporary = tempfile::TempDir::new()?;
    let db = open(temporary.path().join("vectors.git"))?;
    let docs = db.collection("docs");
    docs.upsert([
        Point::new("east", [1.0, 0.0]),
        Point::new("north", [0.0, 1.0]),
    ])?;
    let hits = docs.search([0.9, 0.1], 1)?;
    assert_eq!(hits[0].id.to_string(), "east");
    Ok(())
}
