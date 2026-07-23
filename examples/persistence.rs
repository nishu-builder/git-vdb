use git_vdb::{open, Point};

fn main() -> git_vdb::Result<()> {
    let temporary = tempfile::TempDir::new()?;
    let path = temporary.path().join("vectors.git");
    open(&path)?
        .collection("docs")
        .upsert([Point::new("stable-id", [1.0, 0.0])])?;
    assert_eq!(open(&path)?.collection("docs").count()?, 1);
    Ok(())
}
