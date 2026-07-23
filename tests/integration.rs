use git_vdb::{
    CollectionConfig, Condition, Database, DeleteSelector, Filter, GetRequest, Point, PointId,
};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

fn point(id: &str, vector: [f32; 2], payload: Value) -> Point {
    Point {
        id: id.into(),
        vector: vector.into(),
        payload: payload.as_object().unwrap().clone(),
    }
}

#[test]
fn incremental_mutation_matches_clean_rebuild_and_diff_reports_reuse() {
    let temp = TempDir::new().unwrap();
    let clean_temp = TempDir::new().unwrap();
    let config = CollectionConfig {
        dimension: 2,
        ..CollectionConfig::default()
    };
    let db = Database::init(temp.path()).unwrap();
    let incremental = db.create_collection("points", config.clone()).unwrap();
    incremental
        .upsert(vec![
            point("a", [1.0, 0.0], json!({"kind": "keep"})),
            point("b", [0.0, 1.0], json!({"kind": "remove"})),
        ])
        .unwrap();
    let before = incremental.root().unwrap();
    incremental
        .upsert(vec![point("a", [0.9, 0.1], json!({"kind": "changed"}))])
        .unwrap();
    let final_write = incremental
        .delete(DeleteSelector {
            ids: vec![PointId::from("b")],
            ..DeleteSelector::default()
        })
        .unwrap();

    let clean_db = Database::init_bare(clean_temp.path()).unwrap();
    let clean = clean_db.create_collection("other-name", config).unwrap();
    let clean_root = clean
        .upsert(vec![point("a", [0.9, 0.1], json!({"kind": "changed"}))])
        .unwrap()
        .root;
    assert_eq!(final_write.root, clean_root);

    let diff = incremental.diff(&before, &final_write.root).unwrap();
    assert_eq!(diff.removed, vec![PointId::from("b")]);
    assert_eq!(diff.changed, vec![PointId::from("a")]);
    assert!(diff.shared.objects > 0);
    assert!(diff.left_unique.objects > 0);
    assert!(diff.right_unique.objects > 0);
}

#[test]
fn filters_get_delete_and_count_use_typed_ids_and_dot_paths() {
    let temp = TempDir::new().unwrap();
    let db = Database::init(temp.path()).unwrap();
    let collection = db
        .create_collection(
            "c",
            CollectionConfig {
                dimension: 2,
                ..CollectionConfig::default()
            },
        )
        .unwrap();
    collection
        .upsert(vec![
            point("1", [1.0, 0.0], json!({"meta": {"year": 2026}})),
            Point {
                id: 1_u64.into(),
                vector: vec![0.0, 1.0],
                payload: json!({"meta": {"year": 2024}}).as_object().unwrap().clone(),
            },
        ])
        .unwrap();
    let filter: Filter = serde_json::from_value(json!({
        "must": [{"key": "meta.year", "range": {"gte": 2025}}]
    }))
    .unwrap();
    assert_eq!(collection.count(Some(filter.clone())).unwrap().count, 1);
    let result = collection
        .get(GetRequest {
            filter: Some(Filter {
                must: vec![Condition::has_id([PointId::from("1")])],
                ..Filter::default()
            }),
            with_payload: true,
            ..GetRequest::default()
        })
        .unwrap();
    assert_eq!(result.points.len(), 1);
    assert_eq!(result.points[0].id, PointId::from("1"));
    collection
        .delete(DeleteSelector {
            filter: Some(filter),
            ..DeleteSelector::default()
        })
        .unwrap();
    assert_eq!(collection.count(None).unwrap().count, 1);
}

#[test]
fn full_validation_rejects_a_deliberately_corrupted_index() {
    let temp = TempDir::new().unwrap();
    let db = Database::init(temp.path()).unwrap();
    let collection = db
        .create_collection(
            "c",
            CollectionConfig {
                dimension: 2,
                ..CollectionConfig::default()
            },
        )
        .unwrap();
    collection
        .upsert(vec![point("a", [1.0, 0.0], json!({}))])
        .unwrap();

    let repository = git2::Repository::open(temp.path()).unwrap();
    let root_id = git2::Oid::from_str(&collection.root().unwrap().0).unwrap();
    let root = repository.find_tree(root_id).unwrap();
    let empty = repository.treebuilder(None).unwrap().write().unwrap();
    let mut index = repository.treebuilder(None).unwrap();
    index.insert("lsh-v1", empty, 0o040000).unwrap();
    let index = index.write().unwrap();
    let mut corrupt = repository.treebuilder(Some(&root)).unwrap();
    corrupt.insert("index", index, 0o040000).unwrap();
    let corrupt_root = corrupt.write().unwrap().to_string();
    drop(corrupt);
    drop(root);
    assert!(collection.at(corrupt_root).unwrap().validate(true).is_err());
}

#[test]
fn cli_outputs_json_and_stock_git_can_transfer_and_maintain_objects() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("db");
    let binary = env!("CARGO_BIN_EXE_git-vdb");
    assert_success(Command::new(binary).args(["init", repo.to_str().unwrap()]));
    let created = assert_success(Command::new(binary).args([
        "--repo",
        repo.to_str().unwrap(),
        "collection",
        "create",
        "notes",
        "--dimension",
        "2",
    ]));
    let created: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(created["point_count"], 0);
    assert_eq!(created["format_version"], 2);

    let input = temp.path().join("points.jsonl");
    fs::write(
        &input,
        "{\"id\":\"a\",\"vector\":[1.0,0.0],\"payload\":{\"topic\":\"rust\"}}\n\
         {\"id\":7,\"vector\":[0.0,1.0],\"payload\":{}}\n",
    )
    .unwrap();
    let upserted = assert_success(Command::new(binary).args([
        "--repo",
        repo.to_str().unwrap(),
        "upsert",
        "notes",
        "--input",
        input.to_str().unwrap(),
    ]));
    let upserted: Value = serde_json::from_slice(&upserted.stdout).unwrap();
    let root = upserted["root"].as_str().unwrap();

    let tree = git(&repo, &["ls-tree", "--name-only", root]);
    assert_eq!(tree, "index\nmeta.json\npoints\n");
    assert_eq!(
        git(&repo, &["ls-tree", "--name-only", &format!("{root}:index")]),
        "ivf-flat-v2\n"
    );
    let ref_before = git(&repo, &["rev-parse", "refs/git-vdb/collections/notes"]);
    let object_count_before = git(&repo, &["count-objects", "-v"]);
    let vector_file = temp.path().join("query.json");
    fs::write(&vector_file, "[1.0,0.0]\n").unwrap();
    let queried = assert_success(Command::new(binary).args([
        "--repo",
        repo.to_str().unwrap(),
        "query",
        "notes",
        "--vector",
        vector_file.to_str().unwrap(),
        "--exact",
    ]));
    let queried: Value = serde_json::from_slice(&queried.stdout).unwrap();
    assert_eq!(queried["root"], root);
    assert_eq!(queried["points"][0]["id"], "a");
    assert_eq!(
        ref_before,
        git(&repo, &["rev-parse", "refs/git-vdb/collections/notes"])
    );
    assert_eq!(object_count_before, git(&repo, &["count-objects", "-v"]));

    let remote = temp.path().join("remote.git");
    assert!(Command::new("git")
        .args(["init", "--bare", remote.to_str().unwrap()])
        .output()
        .unwrap()
        .status
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args([
            "push",
            remote.to_str().unwrap(),
            "refs/git-vdb/collections/notes:refs/git-vdb/collections/notes",
        ])
        .output()
        .unwrap()
        .status
        .success());
    assert_success(
        Command::new("git")
            .arg("--git-dir")
            .arg(&remote)
            .args(["cat-file", "-t", root]),
    );
    assert_success(
        Command::new("git")
            .arg("--git-dir")
            .arg(&remote)
            .args(["repack", "-ad"]),
    );
    assert_success(
        Command::new("git")
            .arg("--git-dir")
            .arg(&remote)
            .args(["gc", "--prune=now"]),
    );
    assert_success(
        Command::new("git")
            .arg("--git-dir")
            .arg(&remote)
            .args(["cat-file", "-t", root]),
    );
}

#[test]
fn cli_first_use_auto_creates_and_accepts_files_stdin_and_inline_vectors() {
    let temp = TempDir::new().unwrap();
    let binary = env!("CARGO_BIN_EXE_git-vdb");
    let missing = temp.path().join("missing.git");
    let failed_read = Command::new(binary)
        .args(["--db", missing.to_str().unwrap(), "count", "documents"])
        .output()
        .unwrap();
    assert!(!failed_read.status.success());
    assert!(!missing.exists());

    let repo = temp.path().join("vectors.git");
    let input = temp.path().join("points.jsonl");
    fs::write(
        &input,
        "{\"id\":\"east\",\"vector\":[1.0,0.0],\"payload\":{\"label\":\"East\"}}\n",
    )
    .unwrap();
    assert_success(Command::new(binary).args([
        "--db",
        repo.to_str().unwrap(),
        "upsert",
        "documents",
        input.to_str().unwrap(),
    ]));
    assert_success(Command::new(binary).args([
        "--db",
        repo.to_str().unwrap(),
        "upsert",
        "documents",
        "--id",
        "north",
        "--vector",
        "0,1",
        "--payload",
        "{\"label\":\"North\"}",
    ]));

    let mut child = Command::new(binary)
        .args(["--db", repo.to_str().unwrap(), "upsert", "documents", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"id\":\"west\",\"vector\":[-1.0,0.0],\"payload\":{}}\n")
        .unwrap();
    let stdin_write = child.wait_with_output().unwrap();
    assert!(stdin_write.status.success(), "{stdin_write:?}");

    let searched = assert_success(Command::new(binary).args([
        "--db",
        repo.to_str().unwrap(),
        "search",
        "documents",
        "--vector",
        "0.9,0.1",
        "--limit",
        "2",
        "--with-payload",
    ]));
    let searched: Value = serde_json::from_slice(&searched.stdout).unwrap();
    assert_eq!(searched["points"][0]["id"], "east");
    assert_eq!(searched["points"][0]["payload"]["label"], "East");

    let counted = assert_success(Command::new(binary).args([
        "--db",
        repo.to_str().unwrap(),
        "count",
        "documents",
    ]));
    let counted: Value = serde_json::from_slice(&counted.stdout).unwrap();
    assert_eq!(counted["count"], 3);
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = assert_success(Command::new("git").arg("-C").arg(repo).args(args));
    String::from_utf8(output.stdout).unwrap()
}

fn assert_success(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "command failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
