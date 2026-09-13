use git_vdb::{CollectionConfig, Document, Embedder, SnapshotEngine, TextQuery};

struct Compass;
impl Embedder for Compass {
    fn model_id(&self) -> &str {
        "example/compass@1"
    }
    fn embed(&self, texts: &[String]) -> git_vdb::Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|text| {
                if text.contains("north") {
                    vec![0.0, 1.0]
                } else {
                    vec![1.0, 0.0]
                }
            })
            .collect())
    }
}

fn main() -> git_vdb::Result<()> {
    let engine = SnapshotEngine::ephemeral()?;
    let text = engine.build_text(
        CollectionConfig::new(2),
        [Document::new("guide", "north wind")],
        Compass,
    )?;
    let root = text.snapshot().root();
    let reopened = engine.open_snapshot(root)?.with_embedder(Compass)?;
    let hits = reopened.query(TextQuery::new("north").limit(1))?;
    assert_eq!(hits[0].document, "north wind");
    Ok(())
}
