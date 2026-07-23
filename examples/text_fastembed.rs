#[cfg(feature = "fastembed")]
use git_vdb::{open, Document, FastEmbedder};

#[cfg(feature = "fastembed")]
fn main() -> git_vdb::Result<()> {
    let temporary = tempfile::TempDir::new()?;
    let db = open(temporary.path().join("documents.git"))?;
    let docs = db.text_collection("docs", FastEmbedder::try_new()?)?;
    docs.upsert_documents([
        Document::new("git", "Git-native vector search"),
        Document::new("fruit", "Fresh fruit at the market"),
    ])?;
    let hits = docs.search_text("versioned database search", 1)?;
    println!("{}", hits[0].id);
    Ok(())
}

#[cfg(not(feature = "fastembed"))]
fn main() {
    eprintln!("run with --features fastembed");
}
