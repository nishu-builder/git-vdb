use git_vdb::{
    open, CollectionConfig, Condition, Database, Error, Filter, GetRequest, Point, PointId, Query,
    SnapshotMutation,
};
use serde_json::json;
use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;
use tempfile::TempDir;

fn points() -> Vec<Point> {
    vec![
        Point::new("east", [1.0, 0.0])
            .with_metadata(json!({"label": "East"}))
            .unwrap(),
        Point::new("north", [0.0, 1.0])
            .with_metadata(json!({"label": "North"}))
            .unwrap(),
    ]
}

#[test]
fn golden_path_opens_upserts_searches_and_reopens() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("vectors.git");
    let store = open(&path).unwrap();
    let docs = store.collection("docs");

    assert!(matches!(docs.count(), Err(Error::CollectionNotFound(_))));
    let write = docs.upsert(points()).unwrap();
    assert_eq!(docs.count().unwrap(), 2);
    let hits = docs.search([0.9, 0.1], 1).unwrap();
    assert_eq!(hits[0].id, PointId::from("east"));
    assert_eq!(hits[0].payload.as_ref().unwrap()["label"], "East");

    let reopened = open(&path).unwrap().collection("docs");
    assert_eq!(reopened.advanced().unwrap().root().unwrap(), write.root);
    assert_eq!(reopened.peek(10).unwrap().len(), 2);
    assert_eq!(reopened.get_ids([PointId::from("north")]).unwrap().len(), 1);
    assert_eq!(
        reopened
            .delete_ids([PointId::from("north")])
            .unwrap()
            .affected_points,
        1
    );
    assert_eq!(reopened.count().unwrap(), 1);
}

#[test]
fn facade_and_advanced_api_produce_the_same_canonical_root() {
    let simple_temp = TempDir::new().unwrap();
    let simple = open(simple_temp.path().join("simple.git")).unwrap();
    let simple_root = simple.collection("docs").upsert(points()).unwrap().root;

    let advanced_temp = TempDir::new().unwrap();
    let advanced = Database::init_bare(advanced_temp.path()).unwrap();
    let collection = advanced
        .create_collection("docs", CollectionConfig::new(2))
        .unwrap();
    let advanced_root = collection.upsert(points()).unwrap().root;

    assert_eq!(simple_root, advanced_root);
}

#[test]
fn open_is_safe_for_existing_paths() {
    let temp = TempDir::new().unwrap();
    let empty = temp.path().join("empty");
    fs::create_dir(&empty).unwrap();
    open(&empty).unwrap();
    assert!(git2::Repository::open_bare(&empty).is_ok());

    let nonbare = temp.path().join("nonbare");
    Database::init(&nonbare).unwrap();
    assert!(open(&nonbare).is_ok());

    let occupied = temp.path().join("occupied");
    fs::create_dir(&occupied).unwrap();
    let sentinel = occupied.join("keep.txt");
    fs::write(&sentinel, "keep").unwrap();
    let error = open(&occupied).unwrap_err();
    assert!(matches!(error, Error::Invalid(_)));
    assert_eq!(fs::read_to_string(sentinel).unwrap(), "keep");
}

#[test]
fn first_write_validates_the_inferred_dimension() {
    let temp = TempDir::new().unwrap();
    let docs = open(temp.path().join("vectors.git"))
        .unwrap()
        .collection("docs");

    assert!(docs.upsert(Vec::<Point>::new()).is_err());
    assert!(docs.upsert([Point::new("empty", [])]).is_err());
    assert!(docs
        .upsert([
            Point::new("two", [1.0, 0.0]),
            Point::new("three", [1.0, 0.0, 0.0]),
        ])
        .is_err());
    assert!(matches!(docs.count(), Err(Error::CollectionNotFound(_))));

    docs.upsert([Point::new("two", [1.0, 0.0])]).unwrap();
    let root = docs.advanced().unwrap().root().unwrap();
    assert!(docs.upsert([Point::new("three", [1.0, 0.0, 0.0])]).is_err());
    assert_eq!(docs.advanced().unwrap().root().unwrap(), root);
    assert_eq!(docs.count().unwrap(), 1);
}

#[test]
fn concurrent_first_writes_do_not_silently_overwrite_each_other() {
    let temp = TempDir::new().unwrap();
    let store = open(temp.path().join("vectors.git")).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let mut writers = Vec::new();

    for (id, vector) in [("east", [1.0, 0.0]), ("north", [0.0, 1.0])] {
        let docs = store.collection("docs");
        let barrier = barrier.clone();
        writers.push(thread::spawn(move || {
            barrier.wait();
            docs.upsert([Point::new(id, vector)])
        }));
    }

    for writer in writers {
        writer.join().unwrap().unwrap();
    }
    let docs = store.collection("docs");
    assert_eq!(docs.count().unwrap(), 2);
    assert_eq!(docs.peek(10).unwrap().len(), 2);
}

#[test]
fn metadata_requires_a_json_object() {
    assert!(Point::new("id", [1.0])
        .with_metadata(json!([1, 2]))
        .is_err());
    assert!(Point::new("id", [1.0])
        .with_metadata(json!({"source": "guide.md"}))
        .is_ok());
}

#[test]
fn facade_supports_filtered_operations_and_atomic_mutations() {
    let temp = TempDir::new().unwrap();
    let docs = open(temp.path().join("vectors.git"))
        .unwrap()
        .collection("docs");
    docs.upsert(points()).unwrap();

    let east = Filter::must([Condition::matches("label", "East")]);
    let result = docs
        .query(
            Query::new([1.0, 0.0], 5)
                .with_filter(east.clone())
                .with_payload(),
        )
        .unwrap();
    assert_eq!(result.points.len(), 1);
    assert_eq!(docs.count_where(east.clone()).unwrap(), 1);
    assert_eq!(
        docs.get(GetRequest {
            filter: Some(east.clone()),
            with_payload: true,
            ..GetRequest::default()
        })
        .unwrap()
        .points
        .len(),
        1
    );

    let before = docs.root().unwrap();
    docs.apply([
        SnapshotMutation::delete_filter(east),
        SnapshotMutation::upsert(
            Point::new("west", [-1.0, 0.0])
                .with_metadata(json!({"label": "West"}))
                .unwrap(),
        ),
    ])
    .unwrap();
    assert_ne!(docs.root().unwrap(), before);
    assert_eq!(docs.count().unwrap(), 2);
    assert!(docs.get_ids([PointId::from("east")]).unwrap().is_empty());
    assert_eq!(docs.get_ids([PointId::from("west")]).unwrap().len(), 1);
}
