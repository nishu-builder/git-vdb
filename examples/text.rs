use git_vdb::{open, Document, Embedder, Result};

#[derive(Clone, Debug)]
struct ExampleEmbedder;

impl Embedder for ExampleEmbedder {
    fn model_id(&self) -> &str {
        "example/compass@1"
    }

    fn embed(&self, input: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(input
            .iter()
            .map(|text| {
                if text.to_lowercase().contains("north") {
                    vec![0.0, 1.0]
                } else {
                    vec![1.0, 0.0]
                }
            })
            .collect())
    }
}

fn main() -> git_vdb::Result<()> {
    let temporary = tempfile::TempDir::new()?;
    let db = open(temporary.path().join("documents.git"))?;
    let docs = db.text_collection("docs", ExampleEmbedder)?;
    docs.upsert_documents([
        Document::new("east", "A document about the east"),
        Document::new("north", "A document about the north"),
    ])?;
    assert_eq!(
        docs.search_text("north wind", 1)?[0].id.to_string(),
        "north"
    );
    Ok(())
}
