use git_vdb::*;
use std::cell::RefCell;
use std::path::Path;
struct Traced {
    directory: DirectorySource,
    paths: RefCell<Vec<String>>,
}
impl SnapshotSource for Traced {
    fn read_blob(&self, path: &str) -> Result<Vec<u8>> {
        self.paths.borrow_mut().push(path.into());
        self.directory.read_blob(path)
    }
    fn list(&self, path: &str) -> Result<Vec<String>> {
        self.directory.list(path)
    }
}
fn points() -> Vec<Point> {
    (0..1536)
        .map(|i| {
            let vector = (0..16)
                .map(|j| {
                    if j == i % 16 {
                        1.0
                    } else {
                        ((i * 17 + j * 31) % 97) as f32 / 9700.0
                    }
                })
                .collect::<Vec<_>>();
            Point::new(
                if i % 2 == 0 {
                    PointId::UInt(i as u64)
                } else {
                    PointId::String(format!("doc-{i}"))
                },
                vector,
            )
            .with_metadata(
                serde_json::json!({"group":i%3, "document":"source excerpt ".repeat(60)}),
            )
            .unwrap()
        })
        .collect()
}
fn reader(path: &Path, root: ObjectId) -> SnapshotReader<Traced> {
    SnapshotReader::open(
        root,
        Traced {
            directory: DirectorySource::new(path).unwrap(),
            paths: RefCell::new(vec![]),
        },
    )
    .unwrap()
}
#[test]
fn selective_queries_match_existing_reader_and_avoid_unneeded_objects() {
    let engine = SnapshotEngine::ephemeral().unwrap();
    let snapshot = engine
        .build(
            CollectionConfig::new(16).with_vector_space("fixture"),
            points(),
        )
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    snapshot.materialize(temp.path().join("snapshot")).unwrap();
    let path = temp.path().join("snapshot");
    for exact in [true, false] {
        for filtered in [false, true] {
            for limit in [0, 1, 10] {
                for probes in [0, 1, 64] {
                    let mut query = Query::new(
                        [
                            1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
                            0.0, 0.0,
                        ],
                        limit,
                    )
                    .with_payload()
                    .with_vector()
                    .in_vector_space("fixture");
                    query.params = QueryParams {
                        exact: Some(exact),
                        probes,
                        candidate_limit: 23,
                    };
                    if filtered {
                        query.filter = Some(Filter::must([Condition::matches("group", 1)]));
                    }
                    let expected = snapshot.query(query.clone()).unwrap();
                    let mut lazy = reader(&path, snapshot.root());
                    let actual = lazy.query(query).unwrap();
                    assert_eq!(
                        serde_json::to_value(actual).unwrap(),
                        serde_json::to_value(expected).unwrap(),
                        "exact={exact} filtered={filtered} limit={limit} probes={probes}"
                    );
                    assert!(!lazy
                        .source()
                        .paths
                        .borrow()
                        .iter()
                        .any(|p| p.ends_with("sample.bin")));
                }
            }
        }
    }
    let mut lazy = reader(&path, snapshot.root());
    let mut query = Query::approximate(points()[0].vector.clone(), 10);
    query.params.probes = 1;
    query.params.candidate_limit = 10;
    lazy.query(query).unwrap();
    let paths = lazy.source().paths.borrow();
    assert!(!paths.iter().any(|p| p.contains("/payloads/")));
    assert_eq!(paths.iter().filter(|p| p.contains("/postings/")).count(), 1);
    assert!(lazy.read_stats().blob_bytes < 200_000);
    let before = lazy.read_stats();
    drop(paths);
    assert!(lazy
        .query(Query::new([1.0; 16], 1).in_vector_space("wrong"))
        .is_err());
    assert_eq!(before, lazy.read_stats());
}
#[test]
fn empty_and_tied_vectors_keep_exact_order() {
    for points in [
        vec![],
        vec![Point::new(1_u64, [0.0, 0.0]), Point::new("1", [0.0, 0.0])],
    ] {
        let engine = SnapshotEngine::ephemeral().unwrap();
        let snapshot = engine.build(CollectionConfig::new(2), points).unwrap();
        let temp = tempfile::tempdir().unwrap();
        snapshot.materialize(temp.path().join("snapshot")).unwrap();
        for exact in [true, false] {
            let mut query = Query::new([0.0, 0.0], 10).with_payload();
            query.params.exact = Some(exact);
            assert_eq!(
                serde_json::to_value(
                    reader(&temp.path().join("snapshot"), snapshot.root())
                        .query(query.clone())
                        .unwrap()
                )
                .unwrap(),
                serde_json::to_value(snapshot.query(query).unwrap()).unwrap()
            );
        }
    }
}
#[test]
fn accessed_corruption_is_rejected_and_unvisited_payloads_stay_unread() {
    let snapshot = SnapshotEngine::ephemeral()
        .unwrap()
        .build(CollectionConfig::new(16), points())
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    snapshot.materialize(temp.path().join("snapshot")).unwrap();
    let path = temp.path().join("snapshot");
    std::fs::write(path.join("index/ivf-flat-v2/codebook.bin"), b"broken").unwrap();
    assert!(reader(&path, snapshot.root())
        .query(Query::approximate([1.0; 16], 1))
        .is_err());
    // Exact queries do not depend on the approximate codebook.
    assert!(reader(&path, snapshot.root())
        .query(Query::exact([1.0; 16], 1))
        .is_ok());
}

#[test]
fn local_git_source_reads_the_same_immutable_root() {
    let temp = tempfile::tempdir().unwrap();
    let repository = temp.path().join("objects.git");
    let snapshot = SnapshotEngine::init(&repository)
        .unwrap()
        .build(CollectionConfig::new(16), points())
        .unwrap();
    let mut reader = SnapshotReader::open(
        snapshot.root(),
        GitSource::new(&repository, &snapshot.root()).unwrap(),
    )
    .unwrap();
    let query = Query::approximate([1.0; 16], 10).with_payload();
    assert_eq!(
        serde_json::to_value(reader.query(query.clone()).unwrap()).unwrap(),
        serde_json::to_value(snapshot.query(query).unwrap()).unwrap()
    );
    assert_eq!(
        git2::Repository::open(&repository)
            .unwrap()
            .references()
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn near_ties_rank_before_public_f32_rounding() {
    let temp = tempfile::tempdir().unwrap();
    let snapshot = SnapshotEngine::ephemeral()
        .unwrap()
        .build(
            CollectionConfig::new(2),
            vec![
                Point::new("a", [1.0, 0.00002]),
                Point::new("b", [1.0, 0.00001]),
            ],
        )
        .unwrap();
    let directory = temp.path().join("snapshot");
    snapshot.materialize(&directory).unwrap();
    for exact in [true, false] {
        let mut query = Query::new([1.0, 0.0], 2);
        query.params.exact = Some(exact);
        query.params.probes = 64;
        let result = reader(&directory, snapshot.root()).query(query).unwrap();
        assert_eq!(result.points[0].id, PointId::from("b"));
        assert_eq!(result.points[0].score, result.points[1].score);
    }
}
