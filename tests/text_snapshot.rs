use git_vdb::{
    CollectionConfig, Document, Embedder, Error, PointId, Snapshot, SnapshotEngine, TextQuery,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(Clone)]
struct Compass {
    identity: &'static str,
    calls: Arc<AtomicUsize>,
}
impl Compass {
    fn new(identity: &'static str) -> Self {
        Self {
            identity,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}
impl Embedder for Compass {
    fn model_id(&self) -> &str {
        self.identity
    }
    fn embed(&self, input: &[String]) -> git_vdb::Result<Vec<Vec<f32>>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(input
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

#[test]
fn text_snapshots_reopen_mutate_and_retain_history_without_refs() {
    let temp = tempfile::tempdir().unwrap();
    let engine = SnapshotEngine::init(temp.path()).unwrap();
    let provider = Compass::new("compass@weights-tokenizer-runtime-v1");
    let old = engine
        .build_text(
            CollectionConfig::new(2),
            [
                Document::new("a", "east wind"),
                Document::new("b", "north wind")
                    .with_metadata(serde_json::json!({"file":"b.txt"}))
                    .unwrap(),
            ],
            provider.clone(),
        )
        .unwrap();
    let root = old.snapshot().root();
    let reopened = engine
        .open_snapshot(&root)
        .unwrap()
        .with_embedder(provider.clone())
        .unwrap();
    let hits = reopened.query(TextQuery::new("north").limit(1)).unwrap();
    assert_eq!(hits[0].id, PointId::from("b"));
    assert_eq!(hits[0].metadata["file"], "b.txt");

    let next = reopened
        .upsert_documents([Document::new("b", "east breeze")])
        .unwrap();
    assert_ne!(next.root(), root);
    let rebuilt = engine
        .build_text(
            CollectionConfig::new(2),
            [
                Document::new("a", "east wind"),
                Document::new("b", "east breeze"),
            ],
            provider.clone(),
        )
        .unwrap();
    assert_eq!(next.root(), rebuilt.snapshot().root());
    assert_eq!(
        old.query(TextQuery::new("north").limit(1)).unwrap()[0].id,
        PointId::from("b")
    );

    let output = tempfile::tempdir().unwrap();
    next.materialize(output.path().join("index")).unwrap();
    let imported = Snapshot::open_directory(output.path().join("index")).unwrap();
    assert_eq!(imported.root(), next.root());
    let deleted = imported
        .with_embedder(provider)
        .unwrap()
        .delete_ids([PointId::from("b")])
        .unwrap();
    assert_eq!(deleted.info().unwrap().point_count, 1);
    assert_eq!(next.info().unwrap().point_count, 2);
    assert_eq!(
        git2::Repository::open_bare(temp.path())
            .unwrap()
            .references()
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn batches_use_one_embedding_call_and_empty_snapshots_have_explicit_space() {
    let engine = SnapshotEngine::ephemeral().unwrap();
    let provider = Compass::new("compass-v1");
    let snapshot = engine
        .build_text(
            CollectionConfig::new(2),
            Vec::<Document>::new(),
            provider.clone(),
        )
        .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        snapshot
            .snapshot()
            .info()
            .unwrap()
            .config
            .vector_space
            .as_deref(),
        Some("compass-v1")
    );
    let queries = snapshot
        .query_batch([TextQuery::new("east"), TextQuery::new("north")])
        .unwrap();
    assert_eq!(queries, vec![vec![], vec![]]);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(snapshot
        .query_batch(Vec::<TextQuery>::new())
        .unwrap()
        .is_empty());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn model_mismatches_fail_before_inference() {
    let engine = SnapshotEngine::ephemeral().unwrap();
    let provider = Compass::new("compass-v1");
    let snapshot = engine
        .build_text(
            CollectionConfig::new(2),
            [Document::new("a", "east")],
            provider,
        )
        .unwrap();
    let wrong = Compass::new("compass-v2");
    assert!(snapshot.snapshot().with_embedder(wrong.clone()).is_err());
    assert!(engine
        .build_text(
            CollectionConfig::new(2).with_vector_space("other"),
            [Document::new("b", "north")],
            wrong.clone()
        )
        .is_err());
    assert_eq!(wrong.calls.load(Ordering::SeqCst), 0);
    assert!(snapshot
        .snapshot()
        .with_embedder(Compass::new(" "))
        .is_err());
}

struct Broken;
impl Embedder for Broken {
    fn model_id(&self) -> &str {
        "broken"
    }
    fn embed(&self, _: &[String]) -> git_vdb::Result<Vec<Vec<f32>>> {
        Ok(vec![vec![f32::NAN, 0.0]])
    }
}
#[test]
fn malformed_document_vectors_are_rejected() {
    let engine = SnapshotEngine::ephemeral().unwrap();
    assert!(matches!(
        engine.build_text(CollectionConfig::new(2), [Document::new("a", "x")], Broken),
        Err(Error::Invalid(_))
    ));
}
