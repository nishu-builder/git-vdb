use git_vdb::{
    CollectionConfig, Database, Point, PointId, Query, QueryParams, Snapshot, SnapshotEngine,
    SnapshotMutation,
};
use serde_json::json;
use tempfile::TempDir;

fn point(id: impl Into<PointId>, vector: [f32; 2], topic: &str) -> Point {
    Point {
        id: id.into(),
        vector: vector.into(),
        payload: json!({"topic": topic}).as_object().unwrap().clone(),
    }
}

fn config() -> CollectionConfig {
    CollectionConfig {
        dimension: 2,
        ..CollectionConfig::default()
    }
}

#[test]
fn ref_free_build_apply_and_query_match_the_collection_adapter() {
    let object_database = TempDir::new().unwrap();
    let engine = SnapshotEngine::init(object_database.path()).unwrap();
    let initial = engine
        .build(
            config(),
            vec![
                point("b", [0.0, 1.0], "remove"),
                point("a", [1.0, 0.0], "keep"),
            ],
        )
        .unwrap();

    let repository = git2::Repository::open_bare(object_database.path()).unwrap();
    assert_eq!(repository.references().unwrap().count(), 0);
    assert!(repository
        .find_commit(initial.root().0.parse().unwrap())
        .is_err());
    let tree = repository
        .find_tree(initial.root().0.parse().unwrap())
        .unwrap();
    let signature = git2::Signature::now("test", "test@example.com").unwrap();
    let commit = repository
        .commit(
            None,
            &signature,
            &signature,
            "not a snapshot root",
            &tree,
            &[],
        )
        .unwrap();
    assert!(engine.open_snapshot(commit.to_string()).is_err());
    assert!(engine.open_snapshot("refs/heads/main").is_err());
    assert_eq!(repository.references().unwrap().count(), 0);

    let next = engine
        .apply(
            initial.root(),
            vec![
                SnapshotMutation::delete_ids([PointId::from("b")]),
                SnapshotMutation::upsert(point("c", [0.8, 0.2], "new")),
            ],
        )
        .unwrap();
    assert_eq!(repository.references().unwrap().count(), 0);
    assert_eq!(initial.info().unwrap().point_count, 2);
    assert_eq!(next.info().unwrap().point_count, 2);

    let result = engine
        .query(
            next.root(),
            Query {
                vector: vec![1.0, 0.0],
                limit: 2,
                params: QueryParams {
                    exact: Some(true),
                    ..QueryParams::default()
                },
                ..Query::default()
            },
        )
        .unwrap();
    assert_eq!(result.root, next.root());
    assert_eq!(result.points[0].id, PointId::from("a"));

    let adapter_repo = TempDir::new().unwrap();
    let database = Database::init(adapter_repo.path()).unwrap();
    let collection = database.create_collection("named", config()).unwrap();
    let adapter_root = collection
        .upsert(vec![
            point("c", [0.8, 0.2], "new"),
            point("a", [1.0, 0.0], "keep"),
        ])
        .unwrap()
        .root;
    assert_eq!(next.root(), adapter_root);
}

#[test]
fn materialized_directory_preserves_root_and_is_independently_queryable() {
    let object_database = TempDir::new().unwrap();
    let engine = SnapshotEngine::init(object_database.path()).unwrap();
    let points = vec![
        point("a", [1.0, 0.0], "rust"),
        point(7_u64, [0.0, 1.0], "git"),
    ];
    let snapshot = engine.build(config(), points.clone()).unwrap();

    let output_parent = TempDir::new().unwrap();
    let directory = output_parent.path().join("snapshot");
    snapshot.materialize(&directory).unwrap();
    assert!(directory.join("meta.json").is_file());
    assert!(directory.join("points").is_dir());
    assert!(directory.join("index/lsh-v1").is_dir());
    assert!(!directory.join(".git").exists());

    let opened = Snapshot::open_directory(&directory).unwrap();
    assert_eq!(opened.root(), snapshot.root());
    assert!(opened.validate(true).unwrap().valid);
    let result = opened
        .query(Query {
            vector: vec![1.0, 0.0],
            params: QueryParams {
                exact: Some(true),
                ..QueryParams::default()
            },
            ..Query::default()
        })
        .unwrap();
    assert_eq!(result.points[0].id, PointId::from("a"));

    let imported_database = TempDir::new().unwrap();
    let imported_engine = SnapshotEngine::init(imported_database.path()).unwrap();
    let imported = imported_engine.import_directory(&directory).unwrap();
    assert_eq!(imported.root(), snapshot.root());

    let next = opened
        .apply(vec![SnapshotMutation::upsert(point(
            "b",
            [0.9, 0.1],
            "new",
        ))])
        .unwrap();
    assert_ne!(next.root(), opened.root());
    assert_eq!(
        Snapshot::open_directory(&directory).unwrap().root(),
        opened.root()
    );

    let rebuilt_directory = output_parent.path().join("rebuilt");
    let rebuilt = SnapshotEngine::build_directory(&rebuilt_directory, config(), points).unwrap();
    assert_eq!(rebuilt.root(), snapshot.root());
}
