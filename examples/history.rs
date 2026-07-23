use git_vdb::{open, Point};

fn main() -> git_vdb::Result<()> {
    let temporary = tempfile::TempDir::new()?;
    let docs = open(temporary.path().join("vectors.git"))?.collection("docs");
    docs.upsert([Point::new("v1", [1.0, 0.0])])?;
    docs.upsert([Point::new("v2", [0.0, 1.0])])?;
    assert_eq!(docs.advanced()?.history(10)?.len(), 3);
    Ok(())
}
