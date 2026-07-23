use git_vdb::{open, Document, Embedder, Error, Result};
use serde_json::json;
use tempfile::TempDir;

#[derive(Clone, Debug)]
struct CompassEmbedder(&'static str);

impl Embedder for CompassEmbedder {
    fn model_id(&self) -> &str {
        self.0
    }

    fn embed(&self, input: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(input
            .iter()
            .map(|text| {
                if text.to_ascii_lowercase().contains("north") {
                    vec![0.0, 1.0]
                } else {
                    vec![1.0, 0.0]
                }
            })
            .collect())
    }
}

#[test]
fn documents_are_embedded_stored_and_queried_offline() {
    let temp = TempDir::new().unwrap();
    let store = open(temp.path().join("documents.git")).unwrap();
    let documents = store
        .text_collection("docs", CompassEmbedder("test/compass@1"))
        .unwrap();
    documents
        .upsert_documents([
            Document::new("east", "A document about the east")
                .with_metadata(json!({"kind": "guide"}))
                .unwrap(),
            Document::new("north", "A document about the north"),
        ])
        .unwrap();

    let hits = documents.search_text("north wind", 1).unwrap();
    assert_eq!(hits[0].id.to_string(), "north");
    assert_eq!(
        hits[0].payload.as_ref().unwrap()["document"],
        "A document about the north"
    );
    assert_eq!(
        documents
            .vectors()
            .advanced()
            .unwrap()
            .info()
            .unwrap()
            .config
            .vector_space
            .as_deref(),
        Some("test/compass@1")
    );
}

#[test]
fn model_identity_is_required_and_cannot_change() {
    let temp = TempDir::new().unwrap();
    let store = open(temp.path().join("documents.git")).unwrap();
    assert!(matches!(
        store.text_collection("docs", CompassEmbedder("")),
        Err(Error::Invalid(_))
    ));
    store
        .text_collection("docs", CompassEmbedder("model/a@1"))
        .unwrap()
        .upsert_documents([Document::new("id", "east")])
        .unwrap();
    assert!(matches!(
        store.text_collection("docs", CompassEmbedder("model/b@1")),
        Err(Error::Invalid(_))
    ));
}

#[derive(Clone, Debug)]
struct BrokenEmbedder;

impl Embedder for BrokenEmbedder {
    fn model_id(&self) -> &str {
        "broken@1"
    }

    fn embed(&self, _: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(Vec::new())
    }
}

#[test]
fn malformed_embedder_output_is_rejected_without_creating_a_collection() {
    let temp = TempDir::new().unwrap();
    let store = open(temp.path().join("documents.git")).unwrap();
    let documents = store.text_collection("docs", BrokenEmbedder).unwrap();
    assert!(documents
        .upsert_documents([Document::new("id", "text")])
        .is_err());
    assert!(documents.vectors().advanced().is_err());
}
